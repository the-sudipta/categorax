#[cfg(windows)]
fn main() {
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("assets/icons/categorax.ico");
    resource.set("ProductName", "Categorax");
    resource.set(
        "FileDescription",
        "Categorax - friendly file and folder tagging",
    );
    resource.set("CompanyName", "Categorax");
    resource.set("LegalCopyright", "Copyright (c) Categorax contributors");
    resource
        .compile()
        .expect("failed to embed Windows application icon");
}

#[cfg(not(windows))]
fn main() {}
