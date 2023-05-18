fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "rpc")]
    {
        println!("cargo:rerun-if-changed=proto/p2p_transport.proto");
        tonic_build::configure()
            .protoc_arg("--experimental_allow_proto3_optional")
            .compile(&["proto/p2p_transport.proto"], &["proto"])?;
    }

    #[cfg(feature = "worker")]
    {
        let cpp_dst =
            cmake::Config::new("./sql-archives/worker").build_target("RustBinding").build();
        println!("cargo:rustc-link-search={}/build/src", cpp_dst.display());
        println!("cargo:rustc-link-lib=static=RustBinding");
        println!("cargo:rerun-if-changed=sql-archives/worker");

        // For some reason linker fails if these worker dependencies are not included directly,
        // even though all relevant symbols should be included in libRustBinding
        let (fmt, spdlog) = match std::env::var("PROFILE").unwrap().as_str() {
            "release" => ("fmt", "spdlog"),
            "debug" => ("fmtd", "spdlogd"),
            x => panic!("Unexpected compilation profile: {x}"),
        };
        println!("cargo:rustc-link-search={}/build/_deps/fmt-build", cpp_dst.display());
        println!("cargo:rustc-link-lib=static={fmt}");
        println!("cargo:rustc-link-search={}/build/_deps/spdlog-build", cpp_dst.display());
        println!("cargo:rustc-link-lib=static={spdlog}");

        cxx_build::bridge("src/worker.rs")
            .file("src/worker/worker.cpp")
            .flag_if_supported("-std=c++20")
            .compile("worker");
        println!("cargo:rerun-if-changed=src/worker");
    }

    Ok(())
}
