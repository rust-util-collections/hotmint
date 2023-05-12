use ruc::*;

use hotmint_types::Block;

pub trait Application: Send + Sync {
    fn create_payload(&self) -> Vec<u8> {
        vec![]
    }
    fn validate_block(&self, _block: &Block) -> bool {
        true
    }
    fn on_commit(&self, block: &Block) -> Result<()>;
}

/// No-op application stub
pub struct NoopApplication;

impl Application for NoopApplication {
    fn on_commit(&self, _block: &Block) -> Result<()> {
        Ok(())
    }
}
