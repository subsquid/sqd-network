#[rustfmt::skip]
fn main() -> std::io::Result<()> {
    println!("cargo:rerun-if-changed=proto/messages.proto");
    prost_build::Config::new()
        .message_attribute(".", "#[derive(Eq, serde::Serialize, serde::Deserialize)]")
        // enum_attribute matches all enums and oneofs, but enums already have Eq implementation
        .enum_attribute(".messages.QueryResult.result", "#[derive(Eq, serde::Serialize, serde::Deserialize)]")
        .enum_attribute(".messages.QueryError.err", "#[derive(Eq, serde::Serialize, serde::Deserialize)]")
        .enum_attribute(".messages.QueryFinished.result", "#[derive(Eq, serde::Serialize, serde::Deserialize)]")
        .enum_attribute(".messages.QueryExecuted.result", "#[derive(Eq, serde::Serialize, serde::Deserialize)]")
        .type_attribute("messages.Range", "#[derive(Copy, Ord, PartialOrd)]")
        .skip_debug(["messages.QueryOk"])
        .field_attribute("messages.QueryOkSummary.data_hash", "#[serde(with = \"hex\")]")
        .field_attribute("messages.Pong.ping_hash", "#[serde(with = \"hex\")]")
        .field_attribute("messages.OldPing.signature","#[serde(with = \"hex\")]")
        .field_attribute("messages.Query.signature", "#[serde(with = \"hex\")]")
        .protoc_arg("--experimental_allow_proto3_optional")
        .compile_protos(&["proto/messages.proto"], &["proto/"])?;
    Ok(())
}
