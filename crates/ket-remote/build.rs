//! Generates the Rust side of `proto/ket/remote/v1/remote.proto`.
//!
//! protox compiles the file in-process, so there is no `protoc` to install;
//! prost turns the descriptors into types.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto");
    let descriptors = protox::compile(["ket/remote/v1/remote.proto"], ["proto"])?;
    prost_build::Config::new().compile_fds(descriptors)?;
    Ok(())
}
