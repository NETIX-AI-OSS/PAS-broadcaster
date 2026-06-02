fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    configure_windows_resources();
}

#[cfg(windows)]
fn configure_windows_resources() {
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("assets/icon.ico");
    resource.set("FileDescription", "PAS Multicast Broadcaster");
    resource.set("ProductName", "PAS Broadcaster");
    resource
        .compile()
        .expect("failed to compile Windows executable resources");
}

#[cfg(not(windows))]
fn configure_windows_resources() {}
