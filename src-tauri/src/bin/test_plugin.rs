//! Small real SDK client for local process-supervision tests.
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--idle") {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    }
    println!(
        "{}",
        json!({"args": &args, "cwd": std::env::current_dir().ok()})
    );
    eprintln!("test_plugin stderr");
    let value = |key: &str| args.windows(2).find(|a| a[0] == key).map(|a| a[1].clone());
    let (Some(port), Some(uuid)) = (value("-port"), value("-pluginUUID")) else {
        return;
    };
    let Ok(port) = port.parse::<u16>() else {
        return;
    };
    let Ok((mut ws, _)) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}")).await
    else {
        return;
    };
    if ws
        .send(
            json!({"event":"registerPlugin","uuid":uuid})
                .to_string()
                .into(),
        )
        .await
        .is_err()
    {
        return;
    }
    while let Some(Ok(message)) = ws.next().await {
        let Ok(text) = message.to_text() else {
            continue;
        };
        let Ok(event) = serde_json::from_str::<Value>(text) else {
            continue;
        };
        if event["event"] == "sendToPlugin" && event["payload"]["exit"] == true {
            std::process::exit(3);
        }
        let response = if event["event"] == "keyDown" {
            Some(json!({"event":"showOk","context":event["context"]}))
        } else if event["event"] == "sendToPlugin" && event["payload"]["spawnChild"] == true {
            let Ok(exe) = std::env::current_exe() else {
                continue;
            };
            let mut command = std::process::Command::new(exe);
            command.arg("--idle");
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x08000000);
            }
            let Ok(child) = command.spawn() else {
                continue;
            };
            Some(
                json!({"event":"childSpawned","childPid":child.id(),"parentPid":std::process::id()}),
            )
        } else {
            None
        };
        if let Some(response) = response {
            if ws.send(response.to_string().into()).await.is_err() {
                break;
            }
        }
    }
}
