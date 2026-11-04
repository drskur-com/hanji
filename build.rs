fn main() {
    // 번역 파일은 `i18n!`이 컴파일할 때 읽는다.
    println!("cargo:rerun-if-changed=locales");
    #[cfg(target_os = "windows")]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("resources/icon/hanji.ico");
        if let Err(e) = res.compile() {
            eprintln!("cargo:warning=Failed to compile Windows resource: {e}");
        }
    }
}
