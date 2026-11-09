fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon/icon.ico");
    // Windows shows the executable's first icon resource on its windows and taskbar button.
    #[cfg(windows)]
    winresource::WindowsResource::new()
        .set_icon("assets/icon/icon.ico")
        .set("ProductName", "Taffrail")
        .set("FileDescription", "Taffrail")
        .compile()
        .expect("failed to embed the Windows icon");
}
