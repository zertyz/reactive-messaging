//! Models whatever const configs are in place

pub enum ConstConfig {
    Atomic(usize),
    _FullSync(usize),
}
impl ConstConfig {
    pub const fn _from(config: usize) -> ConstConfig {
        if config < usize::MAX/2 {
            ConstConfig::Atomic(config)
        } else {
            ConstConfig::_FullSync(config>>32)
        }
    }
    pub const fn into(config: ConstConfig) -> usize {
        match config {
            ConstConfig::Atomic(buffer) => buffer,
            ConstConfig::_FullSync(buffer) => buffer<<32,
        }
    }
}