fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/p2p_transport.proto");
    tonic_build::compile_protos("proto/p2p_transport.proto")?;

    let cpp_dst = cmake::Config::new("./sql-archives/worker").build_target("RustBinding").build();
    println!("cargo:rustc-link-search={}/build/src", cpp_dst.display());
    println!("cargo:rustc-link-lib=RustBinding");
    println!("cargo:rerun-if-changed=sql-archives/worker");

    cxx_build::bridge("src/lib.rs")
        .file("src/worker/worker.cpp")
        .flag_if_supported("-std=c++20")
        .compile("worker");
    println!("cargo:rerun-if-changed=src/worker");

    Ok(())
}
