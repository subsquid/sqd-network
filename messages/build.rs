#[rustfmt::skip]
fn main() -> std::io::Result<()> {
    println!("cargo:rerun-if-changed=proto/messages.proto");
    prost_build::Config::new()
        .type_attribute(".", "#[derive(Eq, serde::Serialize, serde::Deserialize)]")
        .type_attribute("messages.Range", "#[derive(Copy, Ord, PartialOrd)]")
        .field_attribute("messages.SizeAndHash.sha3_256", "#[serde(with = \"hex\")]")
        .field_attribute("messages.QueryExecuted.query_hash","#[serde(with = \"hex\")]")
        .field_attribute("messages.QuerySubmitted.query_hash","#[serde(with = \"hex\")]")
        .field_attribute("messages.Ping.signature","#[serde(with = \"hex\")]")
        .protoc_arg("--experimental_allow_proto3_optional")
        .compile_protos(&["proto/messages.proto"], &["proto/"])?;
    Ok(())
}
