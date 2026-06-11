fn main() {
    if let Err(err) = fracture::run() {
        eprintln!("fracture: {err}");
        std::process::exit(1);
    }
}
