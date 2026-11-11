//! Embeds the Windows icon, manifest and version resource into `gamemode.exe`.
//!
//! The icon makes Explorer, the taskbar and the notification area show the
//! GameMode mark. The manifest requests `asInvoker` so the process starts
//! unelevated and only relaunches through UAC when the user asks for it.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../assets/gamemode.ico");

    let mut res = winres::WindowsResource::new();
    res.set_icon("../assets/gamemode.ico");
    res.set("FileDescription", "GameMode - Gaming Optimiser");
    res.set("ProductName", "GameMode");
    res.set("OriginalFilename", "gamemode.exe");
    res.set("LegalCopyright", "MIT licensed");
    res.set(
        "Comments",
        "Keeps the active game at the front of the CPU, IO and memory queues.",
    );
    res.set_manifest(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="GameMode.App" version="1.0.0.0" processorArchitecture="*"/>
  <description>GameMode - Gaming Optimiser</description>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">permonitorv2</dpiAwareness>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
</assembly>"#,
    );

    if let Err(err) = res.compile() {
        // A missing resource compiler must not break the build; the app still
        // runs, it just falls back to a default icon in Explorer.
        println!("cargo:warning=embedding Windows resources failed: {err}");
    }

    let _ = PathBuf::from(".");
}