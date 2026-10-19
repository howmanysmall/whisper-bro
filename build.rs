fn main() {
    println!("cargo:rerun-if-changed=packaging/yap.ico");

    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("packaging/yap.ico")
            .set("ProductName", "Yap")
            .set("FileDescription", "Yap voice typing")
            .set("LegalCopyright", "Copyright (c) HowManySmall");
        if let Err(error) = resource.compile() {
            panic!("failed to embed Windows resources: {error}");
        }
    }
}
