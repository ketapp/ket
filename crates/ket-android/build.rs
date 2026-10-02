//! Generates the client side of the emulator's control API from
//! `proto/emulator_controller.proto`, as shipped in the Android SDK
//! (`emulator/lib/`). protox compiles it in-process — including the
//! `google/protobuf/empty.proto` it imports — so no `protoc` is needed.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto");
    let descriptors = protox::compile(["emulator_controller.proto"], ["proto"])?;
    tonic_prost_build::configure()
        .build_server(false)
        .compile_fds(descriptors)?;
    Ok(())
}
