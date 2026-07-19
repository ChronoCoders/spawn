//! Relevancy-class marker components. Placing one of these on a `Replicated`
//! entity is what assigns its relevancy class, and, in an archetype ECS, its
//! archetype (validated stable under client churn, see the IM prototype results).
//! These are engine-provided component types; the game inserts them on its entities.

use spawn_ecs::Component;
use spawn_net::ClientId;

/// Gate: only entities carrying `Replicated` are considered for replication at all.
/// An entity with `Replicated` and none of the other markers is a *dynamic spatial*
/// entity (the spatial-grid interest path).
pub struct Replicated;
impl Component for Replicated {}

/// Always relevant to every connected client (match state, rules singletons). Kept in
/// a dedicated list and never spatially tested.
pub struct AlwaysRelevant;
impl Component for AlwaysRelevant {}

/// Relevant only to the owning connection (a player's private state, and the pawn the
/// client controls). Also the ownership anchor for prediction and for the server-RPC
/// gate. Applies independently of [`AlwaysRelevant`].
pub struct OwnerOnly(pub ClientId);
impl Component for OwnerOnly {}

/// Pre-placed and non-moving: its visibility is computed once and then skipped each
/// tick until it changes. Costs ~nothing per tick.
pub struct StaticRelevant;
impl Component for StaticRelevant {}
