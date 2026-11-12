# Plugin fixtures

Downloaded 2026-11-12. These are complete, upstream manifest.json files extracted
from the archives below; only line endings were normalized to LF. No executable
code or image assets are committed. Tests do not access the network and create
placeholder runtime files to test confined runtime resolution.

| Plugin ID | Runtime | Node version | Selected CodePathWin or CodePath | Windows support in manifest |
| --- | --- | --- | --- | --- |
| com.elgato.obsstudio | html | — | source/main.html | Yes |
| com.elgato.spotify | exe | — | Spotify | **No: mac only** |
| com.elgato.discord | exe | — | Discord.exe | Yes |
| com.elgato.cpu | exe | — | cpu.exe | Yes |
| me.amankhanna.oadiscord | exe | — | oadiscord-x86_64-pc-windows-msvc.exe | Yes |
| me.amankhanna.oasystem | exe | — | oasystem-x86_64-pc-windows-msvc.exe | Yes |
| com.ntanis.essentials-for-spotify | node | 20 | bin/plugin.js | Yes |
| de.felitendo.obs-tools | exe | — | bin/x86_64-unknown-linux-gnu/obs-tools | **No: Linux/mac only** |

`exe` means the interface's native-runtime classification; it does not imply a
Windows executable. In particular the two non-Windows fixtures above must not
be assumed usable on Windows. OBS Studio requires the HTML host, so HTML support
cannot be deferred based on the target-plugin investigation.

Sources:

- https://plugins.amankhanna.me/rezipped/com.elgato.obsstudio.zip
- https://plugins.amankhanna.me/rezipped/com.elgato.spotify.zip
- https://plugins.amankhanna.me/rezipped/com.elgato.discord.zip
- https://plugins.amankhanna.me/rezipped/com.elgato.cpu.zip
- https://github.com/OpenActionPlugins/discord/releases/download/v0.5.1/me.amankhanna.oadiscord.zip
- https://github.com/OpenActionPlugins/system/releases/download/v1.0.0/me.amankhanna.oasystem.zip
- https://github.com/ntanis-dev/essentials-for-spotify/releases/download/v1.2.0.0/com.ntanis.essentials-for-spotify.streamDeckPlugin
- https://github.com/FeloDeck/obs-tools/releases/download/v0.3.2/OBS-Tools-v0.3.2.streamDeckPlugin

The four GitHub assets are the latest non-prerelease release assets at download.
Repository IDs were obtained from https://openactionapi.github.io/plugins/catalogue.json.

Compatibility details exposed by these fixtures: CPU FontSize may be a string;
OpenAction OS entries omit MinimumVersion, and System Information states can
contain a title without an Image. The parser accepts all three forms.
