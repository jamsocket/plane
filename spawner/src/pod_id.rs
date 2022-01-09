use convert_base::Convert;
use uuid::Uuid;

const ALPHANUM: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

#[derive(Debug, Clone, PartialEq)]
pub struct PodId(String);

impl PodId {
    pub fn new() -> Self {
        let mut base = Convert::new(256, 36);
        let result = base.convert::<u8, u8>(Uuid::new_v4().as_bytes());
        let ch = result.into_iter().map(|ix| ALPHANUM[ix as usize] as char);

        PodId(ch.collect())
    }

    #[allow(unused)]
    pub fn from_name(name: &str) -> Self {
        PodId(name.to_string())
    }

    pub fn prefixed_name(&self) -> String {
        format!("session-{}", self.0)
    }

    pub fn name(&self) -> String {
        self.0.clone()
    }
}
