use crate::identity::ActorRef;
use crate::ids::{CapabilityId, ExecutionId};
use crate::manifests::PackageKind;

pub trait Identified {
    type Id;
    fn id(&self) -> &Self::Id;
}

pub trait Referencable {
    type Ref;
    fn to_ref(&self) -> Self::Ref;
}

pub trait PackageObject: Identified {
    fn kind(&self) -> PackageKind;
    fn version(&self) -> &str;
    fn summary(&self) -> &str;
}

pub trait GovernedAction {
    fn capability_id(&self) -> &CapabilityId;
    fn execution_id(&self) -> &ExecutionId;
    fn actor(&self) -> &ActorRef;
}
