fn main() {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let proto_dir = manifest_dir.join("../../proto");
    let proto_file = proto_dir.join("abci.proto");
    prost_build::compile_protos(&[proto_file], &[proto_dir]).unwrap();
}
