//! The role triad. A replicated entity's role is *derived* per peer from ownership
//! and locality, it is never stored on the entity.

use spawn_net::ClientId;

/// What a replicated entity is, from a given peer's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetRole {
    /// The authoritative instance, always the server, for a replicated entity.
    Authority,
    /// The owning client's locally-controlled copy, eligible for prediction.
    AutonomousProxy,
    /// A non-owning client's copy, driven by interpolation/extrapolation.
    SimulatedProxy,
}

impl NetRole {
    /// The role of any replicated entity on the server: it holds authority.
    pub fn on_server() -> Self {
        NetRole::Authority
    }

    /// The role of a replicated entity on a client. It is an [`AutonomousProxy`] iff
    /// it is owned by this client (`owner == Some(local)`) *and* prediction is enabled
    /// for it; otherwise a [`SimulatedProxy`].
    ///
    /// [`AutonomousProxy`]: NetRole::AutonomousProxy
    /// [`SimulatedProxy`]: NetRole::SimulatedProxy
    pub fn on_client(owner: Option<ClientId>, local: ClientId, predicted: bool) -> Self {
        if predicted && owner == Some(local) {
            NetRole::AutonomousProxy
        } else {
            NetRole::SimulatedProxy
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_entities_are_authority() {
        assert_eq!(NetRole::on_server(), NetRole::Authority);
    }

    #[test]
    fn client_role_depends_on_ownership_and_prediction() {
        let me = ClientId(1);
        let other = ClientId(2);
        // Owned + predicted -> autonomous.
        assert_eq!(
            NetRole::on_client(Some(me), me, true),
            NetRole::AutonomousProxy
        );
        // Owned but prediction off -> simulated.
        assert_eq!(
            NetRole::on_client(Some(me), me, false),
            NetRole::SimulatedProxy
        );
        // Owned by someone else -> simulated.
        assert_eq!(
            NetRole::on_client(Some(other), me, true),
            NetRole::SimulatedProxy
        );
        // Unowned -> simulated.
        assert_eq!(NetRole::on_client(None, me, true), NetRole::SimulatedProxy);
    }
}
