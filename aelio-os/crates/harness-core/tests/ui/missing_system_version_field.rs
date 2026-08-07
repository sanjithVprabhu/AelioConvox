use harness_core::SystemVersion;

fn main() {
    let _ = SystemVersion {
        op_catalog: [0; 32],
        dialect: [0; 32],
        renderer: [0; 32],
        serialiser: [0; 32],
        intent_schema: [0; 32],
        extractor_model: [0; 32],
        embedding_model: [0; 32],
        triage_model: [0; 32],
    };
}
