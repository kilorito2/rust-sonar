//! Comprueba que se ha indicado el instalador interno (RUST_SONAR_PAYLOAD) e incrusta el icono.
fn main() {
    println!("cargo:rerun-if-env-changed=RUST_SONAR_PAYLOAD");
    match std::env::var("RUST_SONAR_PAYLOAD") {
        Ok(path) => println!("cargo:rerun-if-changed={path}"),
        Err(_) => panic!("Falta RUST_SONAR_PAYLOAD (ruta del instalador de Inno). Compila con installer/build.ps1."),
    }
    println!("cargo:rerun-if-changed=../../assets/rust_sonar.ico");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/rust_sonar.ico")
        .set("ProductName", "Rust Sonar")
        .set("FileDescription", "Instalador de Rust Sonar")
        .set("CompanyName", "kilorito");
    if let Err(e) = res.compile() {
        println!("cargo:warning=no se pudo incrustar el icono: {e}");
    }
}
