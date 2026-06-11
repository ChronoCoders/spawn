use std::error::Error;
use std::fmt;

use spawn_engine::EngineError;
use spawn_physics::PhysicsError;

#[derive(Debug)]
pub enum FractureError {
    Engine(EngineError),
    Physics(PhysicsError),
}

impl fmt::Display for FractureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Engine(e) => write!(f, "engine error: {e}"),
            Self::Physics(e) => write!(f, "physics error: {e}"),
        }
    }
}

impl Error for FractureError {}

impl From<EngineError> for FractureError {
    fn from(e: EngineError) -> Self {
        Self::Engine(e)
    }
}

impl From<PhysicsError> for FractureError {
    fn from(e: PhysicsError) -> Self {
        Self::Physics(e)
    }
}

pub type FractureResult<T> = Result<T, FractureError>;
