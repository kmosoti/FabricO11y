//! Inverse adjacency over selected attributed relations. The caller chooses the
//! exact predicate; this is not a Wiki syntax, authority, custody or truth claim.
use crate::{
    Predicate, RelationDigest, RevisionDigest,
    as_of::{AsOfView, MAX_VIEW_RELATIONS},
    portable::{MAX_PORTABLE_BYTES, PortableError, PortableView},
};
use std::collections::BTreeMap;
pub const MAX_OUTPUT_BYTES: usize = 256 * 1024;
const DIGEST_BYTES: usize = 32;
// Charge the object key once per link even when the map shares it.
const LINK_FIXED_BYTES: usize = 3 * DIGEST_BYTES + 2; // relation, subject, object, two endpoint statuses
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub input_bytes: usize,
    pub selected_relations: usize,
    pub links: usize,
    pub output_bytes: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endpoint {
    Present,
    SelectedMissing,
    Omitted,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub relation: RelationDigest,
    pub subject: RevisionDigest,
    pub subject_state: Endpoint,
    pub object_state: Endpoint,
    pub attributed_to: String,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cost {
    pub selected_relations: usize,
    pub links: usize,
    pub missing_relations: usize,
    pub output_bytes: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Projection {
    pub basis_digest: [u8; 32],
    pub predicate: Predicate,
    pub incoming: BTreeMap<RevisionDigest, Vec<Link>>,
    pub missing_relations: Vec<RelationDigest>,
    pub cost: Cost,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Limit,
    InvalidPredicate,
    Portable(PortableError),
}
fn endpoint(view: &AsOfView, id: &RevisionDigest) -> Endpoint {
    match view.revision(id) {
        Some(Some(_)) => Endpoint::Present,
        Some(None) => Endpoint::SelectedMissing,
        None => Endpoint::Omitted,
    }
}
/// Counts are logical payload bytes; no allocation or completeness claim for
/// relations outside the verified selected view is implied.
pub fn project(
    bytes: &[u8],
    expected: [u8; 32],
    predicate: &Predicate,
    limits: Limits,
) -> Result<Projection, Error> {
    if limits.input_bytes > MAX_PORTABLE_BYTES
        || limits.selected_relations > MAX_VIEW_RELATIONS
        || limits.links > MAX_VIEW_RELATIONS
        || limits.output_bytes > MAX_OUTPUT_BYTES
        || bytes.len() > limits.input_bytes
    {
        return Err(Error::Limit);
    }
    if let Predicate::Custom { namespace, name } = predicate
        && [namespace, name]
            .iter()
            .any(|s| s.is_empty() || s.len() > 256 || s.chars().any(char::is_control))
    {
        return Err(Error::InvalidPredicate);
    }
    // One tag plus length-prefixed namespace and name for custom predicates.
    let predicate_bytes = match predicate {
        Predicate::Custom { namespace, name } => 1 + 2 * 8 + namespace.len() + name.len(),
        _ => 1,
    };
    let portable = PortableView::import(bytes, expected).map_err(Error::Portable)?;
    let view = portable.view();
    if view.relations().len() > limits.selected_relations {
        return Err(Error::Limit);
    }
    let mut cost = Cost {
        selected_relations: view.relations().len(),
        output_bytes: predicate_bytes,
        ..Default::default()
    };
    for relation in view.relations().values() {
        match relation {
            None => {
                cost.missing_relations =
                    cost.missing_relations.checked_add(1).ok_or(Error::Limit)?;
                cost.output_bytes = cost.output_bytes.checked_add(32).ok_or(Error::Limit)?;
            }
            Some(r) if r.predicate == *predicate => {
                cost.links = cost.links.checked_add(1).ok_or(Error::Limit)?;
                cost.output_bytes = cost
                    .output_bytes
                    .checked_add(LINK_FIXED_BYTES)
                    .and_then(|n| n.checked_add(r.attributed_to.len()))
                    .ok_or(Error::Limit)?;
            }
            _ => (),
        }
    }
    if cost.links > limits.links || cost.output_bytes > limits.output_bytes {
        return Err(Error::Limit);
    }
    let mut incoming: BTreeMap<RevisionDigest, Vec<Link>> = BTreeMap::new();
    let mut missing_relations = Vec::with_capacity(cost.missing_relations);
    for (id, relation) in view.relations() {
        match relation {
            None => missing_relations.push(*id),
            Some(r) if r.predicate == *predicate => {
                incoming.entry(r.object).or_default().push(Link {
                    relation: *id,
                    subject: r.subject,
                    subject_state: endpoint(view, &r.subject),
                    object_state: endpoint(view, &r.object),
                    attributed_to: r.attributed_to.clone(),
                })
            }
            _ => (),
        }
    }
    Ok(Projection {
        basis_digest: expected,
        predicate: predicate.clone(),
        incoming,
        missing_relations,
        cost,
    })
}
