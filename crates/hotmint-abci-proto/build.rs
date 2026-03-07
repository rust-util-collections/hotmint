fn main() {
    prost_build::compile_protos(&["../../proto/abci.proto"], &["../../proto/"]).unwrap();
}
