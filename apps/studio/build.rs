// Puts the application icon into the Windows executable (what Explorer and the taskbar show for the file).
fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/app.rc");
        println!("cargo:rerun-if-changed=assets/icon.ico");
        embed_resource::compile("assets/app.rc", embed_resource::NONE).manifest_optional().unwrap();
    }
}
