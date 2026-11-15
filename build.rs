//! Incrusta en el .exe el icono (el que se ve en el Explorador, el menú Inicio y el instalador) y
//! los datos de versión de Windows. El icono se genera con `installer/make_icon.py`.

fn main() {
    println!("cargo:rerun-if-changed=assets/rust_sonar.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/rust_sonar.ico")
            .set("ProductName", "Rust Sonar")
            .set("FileDescription", "Rust Sonar - mezclador de audio")
            .set("CompanyName", "kilorito")
            .set("LegalCopyright", "Proyecto universitario");
        if let Err(e) = res.compile() {
            // Sin el SDK de Windows (rc.exe) se compila igual, solo que sin icono en el .exe.
            println!("cargo:warning=no se pudo incrustar el icono: {e}");
        }
    }
}
