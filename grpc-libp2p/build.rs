fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/worker_rpc.proto");
    tonic_build::compile_protos("proto/worker_rpc.proto")?;
    Ok(())
}
