//! Embeds the application icon and version info into the Windows executable.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    // Only when building *on* Windows for Windows (needs the resource compiler).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") && cfg!(windows) {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon.ico")
            .set("FileDescription", "OpenDeckN3")
            .set("ProductName", "OpenDeckN3");
        if let Err(err) = res.compile() {
            println!("cargo:warning=could not embed Windows resources: {err}");
        }
    }
}
