//! Stream Deck SDK v2 messages (https://docs.elgato.com/streamdeck/sdk/).

use serde_json::{json, Value};

/// Version reported to plugins as the host application version.
pub const HOST_VERSION: &str = "6.5.0";

/// Messages from plugins and property inspectors. Unknown events never fail the connection.
#[derive(Debug, Clone, PartialEq)]
pub enum Inbound {
    RegisterPlugin { uuid: String },
    RegisterPropertyInspector { uuid: String },
    SetSettings { context: String, payload: Value },
    GetSettings { context: String },
    SetGlobalSettings { context: String, payload: Value },
    GetGlobalSettings { context: String },
    SetTitle { context: String, title: Option<String>, state: Option<u8> },
    SetImage { context: String, image: Option<String>, state: Option<u8> },
    SetState { context: String, state: u8 },
    ShowOk { context: String },
    ShowAlert { context: String },
    OpenUrl { url: String },
    LogMessage { message: String },
    SwitchToProfile { profile: Option<String> },
    SendToPropertyInspector { context: String, payload: Value },
    SendToPlugin { context: String, payload: Value },
    /// Valid JSON with an event we do not implement.
    Unknown,
    /// Not JSON, no event, or a known event with missing required fields.
    Invalid,
}

pub fn parse_inbound(text: &str) -> Inbound {
    let Ok(v) = serde_json::from_str::<Value>(text) else { return Inbound::Invalid };
    parse_value(&v).unwrap_or(Inbound::Invalid)
}

fn parse_value(v: &Value) -> Option<Inbound> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(String::from);
    let ctx = || s("context");
    let payload = || v.get("payload").cloned().unwrap_or(Value::Null);
    let p_str = |k: &str| v.get("payload").and_then(|p| p.get(k)).and_then(Value::as_str).map(String::from);
    let p_u8 = |k: &str| v.get("payload").and_then(|p| p.get(k)).and_then(Value::as_u64).map(|n| n.min(255) as u8);
    Some(match v.get("event")?.as_str()? {
        "registerPlugin" => Inbound::RegisterPlugin { uuid: s("uuid")? },
        "registerPropertyInspector" => Inbound::RegisterPropertyInspector { uuid: s("uuid")? },
        "setSettings" => Inbound::SetSettings { context: ctx()?, payload: payload() },
        "getSettings" => Inbound::GetSettings { context: ctx()? },
        "setGlobalSettings" => Inbound::SetGlobalSettings { context: ctx()?, payload: payload() },
        "getGlobalSettings" => Inbound::GetGlobalSettings { context: ctx()? },
        "setTitle" => Inbound::SetTitle { context: ctx()?, title: p_str("title"), state: p_u8("state") },
        "setImage" => Inbound::SetImage { context: ctx()?, image: p_str("image").filter(|i| !i.is_empty()), state: p_u8("state") },
        "setState" => Inbound::SetState { context: ctx()?, state: p_u8("state")? },
        "showOk" => Inbound::ShowOk { context: ctx()? },
        "showAlert" => Inbound::ShowAlert { context: ctx()? },
        "openUrl" => Inbound::OpenUrl { url: p_str("url")? },
        "logMessage" => Inbound::LogMessage { message: p_str("message").unwrap_or_default() },
        "switchToProfile" => Inbound::SwitchToProfile { profile: p_str("profile") },
        "sendToPropertyInspector" => Inbound::SendToPropertyInspector { context: ctx()?, payload: payload() },
        "sendToPlugin" => Inbound::SendToPlugin { context: ctx()?, payload: payload() },
        _ => Inbound::Unknown,
    })
}

/// SDK coordinates of a logical key (1 = top-left, row-major, 5 columns).
pub fn coordinates(key: u8) -> Value {
    let k = key.saturating_sub(1);
    json!({ "column": k % 5, "row": k / 5 })
}

pub const DEVICE_NAME: &str = "Fifine D6";

fn device_info(device: &str) -> Value {
    json!({ "id": device, "name": DEVICE_NAME, "type": 0, "size": { "columns": 5, "rows": 3 } })
}

/// The `-info` argument / `info` passed to `connectElgatoStreamDeckSocket`.
pub fn info_json(device: &str, plugin_uuid: &str, plugin_version: &str) -> Value {
    json!({
        "application": { "font": "Segoe UI", "language": "cs", "platform": "windows", "platformVersion": "10.0", "version": HOST_VERSION },
        "plugin": { "uuid": plugin_uuid, "version": plugin_version },
        "devicePixelRatio": 1,
        "colors": {
            "buttonPressedBackgroundColor": "#303030FF", "buttonPressedBorderColor": "#646464FF",
            "buttonPressedTextColor": "#969696FF", "disabledColor": "#F7821B59",
            "highlightColor": "#F7821BFF", "mouseDownColor": "#CF6304FF"
        },
        "devices": [device_info(device)]
    })
}

/// One action instance on a key.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionCtx {
    pub action: String,
    pub context: String,
    pub device: String,
    pub key: u8,
    pub settings: Value,
    pub state: u8,
}

fn base(event: &str, c: &ActionCtx, payload: Value) -> Value {
    json!({ "event": event, "action": c.action, "context": c.context, "device": c.device, "payload": payload })
}

/// `keyDown` / `keyUp`.
pub fn key_event(event: &str, c: &ActionCtx) -> Value {
    base(event, c, json!({ "settings": c.settings, "coordinates": coordinates(c.key), "controller": "Keypad", "state": c.state, "isInMultiAction": false }))
}

/// `willAppear` / `willDisappear`.
pub fn appear_event(event: &str, c: &ActionCtx) -> Value {
    base(event, c, json!({ "settings": c.settings, "coordinates": coordinates(c.key), "controller": "Keypad", "state": c.state, "isInMultiAction": false }))
}

pub fn did_receive_settings(c: &ActionCtx) -> Value {
    base("didReceiveSettings", c, json!({ "settings": c.settings, "coordinates": coordinates(c.key), "controller": "Keypad", "isInMultiAction": false }))
}

pub fn title_parameters_did_change(c: &ActionCtx, title: &str, show: bool, size: u32, color: &str, align: &str) -> Value {
    base("titleParametersDidChange", c, json!({
        "settings": c.settings, "coordinates": coordinates(c.key), "state": c.state, "title": title,
        "titleParameters": { "fontFamily": "", "fontSize": size, "fontStyle": "", "fontUnderline": false, "showTitle": show, "titleAlignment": align, "titleColor": color }
    }))
}

pub fn pi_event(event: &str, c: &ActionCtx) -> Value {
    json!({ "event": event, "action": c.action, "context": c.context, "device": c.device })
}

/// `sendToPlugin` (PI → plugin) / `sendToPropertyInspector` (plugin → PI), relayed by the host.
pub fn relay(event: &str, action: &str, context: &str, payload: &Value) -> Value {
    json!({ "event": event, "action": action, "context": context, "payload": payload })
}

pub fn did_receive_global_settings(settings: &Value) -> Value {
    json!({ "event": "didReceiveGlobalSettings", "payload": { "settings": settings } })
}

pub fn device_did_connect(device: &str) -> Value {
    json!({ "event": "deviceDidConnect", "device": device, "deviceInfo": device_info(device) })
}

pub fn device_did_disconnect(device: &str) -> Value {
    json!({ "event": "deviceDidDisconnect", "device": device })
}

pub fn application_event(event: &str, application: &str) -> Value {
    json!({ "event": event, "payload": { "application": application } })
}

pub fn system_did_wake_up() -> Value {
    json!({ "event": "systemDidWakeUp" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::json;

    // Required wire fields match OpenAction's MIT-licensed inbound schemas:
    // https://github.com/OpenActionAPI/rust/blob/main/src/inbound/devices.rs
    // https://github.com/OpenActionAPI/rust/blob/main/src/inbound/mod.rs
    // Keep these independent of our builders so missing SDK fields fail decoding.
    #[derive(Deserialize)]
    struct OpenActionDeviceSize {
        rows: u8,
        columns: u8,
    }

    #[derive(Deserialize)]
    struct OpenActionDevice {
        id: String,
        name: String,
        size: OpenActionDeviceSize,
        #[serde(rename = "type")]
        device_type: u8,
    }

    #[derive(Deserialize)]
    struct OpenActionDeviceConnected {
        device: String,
        #[serde(rename = "deviceInfo")]
        device_info: OpenActionDevice,
    }

    #[derive(Deserialize)]
    struct OpenActionInfo {
        devices: Vec<OpenActionDevice>,
    }

    #[derive(Deserialize)]
    struct OpenActionCoordinates {
        row: u8,
        column: u8,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct OpenActionInstancePayload {
        settings: Value,
        coordinates: Option<OpenActionCoordinates>,
        controller: String,
        #[serde(default)]
        state: u16,
        is_in_multi_action: bool,
    }

    #[derive(Deserialize)]
    struct OpenActionInstanceEvent {
        action: String,
        context: String,
        device: String,
        payload: OpenActionInstancePayload,
    }

    #[test]
    fn outbound_devices_decode_with_openaction_required_fields() {
        let connected: OpenActionDeviceConnected =
            serde_json::from_value(device_did_connect("D6-test")).unwrap();
        assert_eq!(connected.device, "D6-test");
        let info: OpenActionInfo =
            serde_json::from_value(info_json("D6-test", "plugin", "1.0")).unwrap();
        assert_eq!(info.devices.len(), 1);
        for device in [&connected.device_info, &info.devices[0]] {
            assert_eq!(device.id, "D6-test");
            assert_eq!(device.name, DEVICE_NAME);
            assert_eq!(device.device_type, 0);
            assert_eq!((device.size.columns, device.size.rows), (5, 3));
        }
    }

    #[test]
    fn outbound_instance_events_decode_with_openaction_required_fields() {
        let ctx = ActionCtx {
            action: "test.action".into(), context: "profile:12".into(),
            device: "D6-test".into(), key: 12,
            settings: json!({"configured": true}), state: 1,
        };
        for value in [
            appear_event("willAppear", &ctx),
            appear_event("willDisappear", &ctx),
            key_event("keyDown", &ctx),
            key_event("keyUp", &ctx),
            did_receive_settings(&ctx),
        ] {
            let event_name = value["event"].as_str().unwrap().to_owned();
            let event: OpenActionInstanceEvent = serde_json::from_value(value)
                .unwrap_or_else(|error| panic!("{event_name}: {error}"));
            assert_eq!(event.action, ctx.action);
            assert_eq!(event.context, ctx.context);
            assert_eq!(event.device, ctx.device);
            assert_eq!(event.payload.settings, ctx.settings);
            assert_eq!(event.payload.controller, "Keypad");
            assert!(!event.payload.is_in_multi_action);
            assert_eq!(event.payload.state, if event_name == "didReceiveSettings" { 0 } else { 1 });
            let coordinates = event.payload.coordinates.unwrap();
            assert_eq!((coordinates.column, coordinates.row), (1, 2));
        }
    }

    fn parse(v: serde_json::Value) -> Inbound {
        parse_inbound(&v.to_string())
    }

    #[test]
    fn registration_messages() {
        assert_eq!(parse(json!({"event": "registerPlugin", "uuid": "abc"})), Inbound::RegisterPlugin { uuid: "abc".into() });
        assert_eq!(parse(json!({"event": "registerPropertyInspector", "uuid": "pi"})), Inbound::RegisterPropertyInspector { uuid: "pi".into() });
    }

    #[test]
    fn plugin_to_host_messages() {
        assert_eq!(
            parse(json!({"event": "setImage", "context": "c1", "payload": {"image": "data:image/png;base64,AA", "target": 0, "state": 1}})),
            Inbound::SetImage { context: "c1".into(), image: Some("data:image/png;base64,AA".into()), state: Some(1) }
        );
        assert_eq!(parse(json!({"event": "setImage", "context": "c1", "payload": {}})), Inbound::SetImage { context: "c1".into(), image: None, state: None });
        assert_eq!(
            parse(json!({"event": "setTitle", "context": "c1", "payload": {"title": "42 %"}})),
            Inbound::SetTitle { context: "c1".into(), title: Some("42 %".into()), state: None }
        );
        assert_eq!(parse(json!({"event": "setState", "context": "c1", "payload": {"state": 1}})), Inbound::SetState { context: "c1".into(), state: 1 });
        assert_eq!(
            parse(json!({"event": "setSettings", "context": "c1", "payload": {"scene": "Game"}})),
            Inbound::SetSettings { context: "c1".into(), payload: json!({"scene": "Game"}) }
        );
        assert_eq!(parse(json!({"event": "getSettings", "context": "c1"})), Inbound::GetSettings { context: "c1".into() });
        assert_eq!(
            parse(json!({"event": "setGlobalSettings", "context": "u", "payload": {"token": "x"}})),
            Inbound::SetGlobalSettings { context: "u".into(), payload: json!({"token": "x"}) }
        );
        assert_eq!(parse(json!({"event": "getGlobalSettings", "context": "u"})), Inbound::GetGlobalSettings { context: "u".into() });
        assert_eq!(parse(json!({"event": "showOk", "context": "c1"})), Inbound::ShowOk { context: "c1".into() });
        assert_eq!(parse(json!({"event": "showAlert", "context": "c1"})), Inbound::ShowAlert { context: "c1".into() });
        assert_eq!(parse(json!({"event": "openUrl", "payload": {"url": "https://x"}})), Inbound::OpenUrl { url: "https://x".into() });
        assert_eq!(parse(json!({"event": "logMessage", "payload": {"message": "hi"}})), Inbound::LogMessage { message: "hi".into() });
        assert_eq!(
            parse(json!({"event": "switchToProfile", "context": "u", "device": "d", "payload": {"profile": "Gaming"}})),
            Inbound::SwitchToProfile { profile: Some("Gaming".into()) }
        );
        assert_eq!(
            parse(json!({"event": "sendToPropertyInspector", "action": "a", "context": "c1", "payload": {"k": 1}})),
            Inbound::SendToPropertyInspector { context: "c1".into(), payload: json!({"k": 1}) }
        );
        assert_eq!(
            parse(json!({"event": "sendToPlugin", "action": "a", "context": "c1", "payload": {"k": 2}})),
            Inbound::SendToPlugin { context: "c1".into(), payload: json!({"k": 2}) }
        );
    }

    #[test]
    fn unknown_and_garbage_do_not_fail() {
        assert_eq!(parse(json!({"event": "setFeedback", "context": "c"})), Inbound::Unknown);
        assert_eq!(parse_inbound("not json"), Inbound::Invalid);
        assert_eq!(parse(json!({"no_event": 1})), Inbound::Invalid);
        assert_eq!(parse(json!({"event": "setState", "context": "c"})), Inbound::Invalid); // missing payload.state
    }

    #[test]
    fn coordinates_are_row_major_from_top_left() {
        assert_eq!(coordinates(1), json!({"column": 0, "row": 0}));
        assert_eq!(coordinates(5), json!({"column": 4, "row": 0}));
        assert_eq!(coordinates(6), json!({"column": 0, "row": 1}));
        assert_eq!(coordinates(15), json!({"column": 4, "row": 2}));
    }

    #[test]
    fn info_json_describes_a_classic_deck() {
        let i = info_json("D6-main", "plug.uuid", "1.2.3");
        assert_eq!(i["application"]["platform"], "windows");
        assert_eq!(i["application"]["version"], "6.5.0");
        assert_eq!(i["plugin"]["uuid"], "plug.uuid");
        assert_eq!(i["plugin"]["version"], "1.2.3");
        assert_eq!(i["devicePixelRatio"], 1);
        assert_eq!(i["devices"][0]["id"], "D6-main");
        assert_eq!(i["devices"][0]["type"], 0);
        assert_eq!(i["devices"][0]["size"], json!({"columns": 5, "rows": 3}));
    }

    #[test]
    fn outbound_key_and_appear_events() {
        let ctx = ActionCtx { action: "com.x.a".into(), context: "p1:3".into(), device: "D6-main".into(), key: 3, settings: json!({"a": 1}), state: 1 };
        let k = key_event("keyDown", &ctx);
        assert_eq!(k["event"], "keyDown");
        assert_eq!(k["action"], "com.x.a");
        assert_eq!(k["context"], "p1:3");
        assert_eq!(k["device"], "D6-main");
        assert_eq!(k["payload"]["settings"], json!({"a": 1}));
        assert_eq!(k["payload"]["coordinates"], json!({"column": 2, "row": 0}));
        assert_eq!(k["payload"]["state"], 1);
        assert_eq!(k["payload"]["isInMultiAction"], false);
        let w = appear_event("willAppear", &ctx);
        assert_eq!(w["payload"]["controller"], "Keypad");
        let s = did_receive_settings(&ctx);
        assert_eq!(s["event"], "didReceiveSettings");
        assert_eq!(s["payload"]["settings"], json!({"a": 1}));
        assert_eq!(device_did_connect("D6-main")["deviceInfo"]["size"]["columns"], 5);
        assert_eq!(did_receive_global_settings(&json!({"t": 1}))["payload"]["settings"]["t"], 1);
    }
}
