//! Deterministic, bounded export of frozen view membership and availability claims.
//! An expected digest must come from an independently trusted channel. Availability
//! labels are assertions; this format does not retrieve or authenticate artifacts.
use crate::as_of::{
    AsOfView, MAX_ATTRIBUTION_BYTES, MAX_GROUP_PREMISES, MAX_TOTAL_PREMISES, MAX_VIEW_GROUPS,
    MAX_VIEW_RELATIONS, MAX_VIEW_REVISIONS, SupportGroup, ViewError,
};
use crate::{
    Corpus, Kind, MAX_CANONICAL_BYTES, MAX_DEPTH, MAX_ITEMS, Predicate, Relation, RevisionBody,
    RevisionDigest, SourceClaim, Value,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const MAGIC: &[u8] = b"fabric-corpus/view\0\x01";
const REVISION_MAGIC: &[u8] = b"fabric-corpus/revision\0\x01";
const RELATION_MAGIC: &[u8] = b"fabric-corpus/relation\0\x01";
pub const MAX_PORTABLE_BYTES: usize = 5 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    Exact,
    Aggregate,
    ReferenceOnly,
    Missing,
}
impl Availability {
    fn tag(self) -> u8 {
        match self {
            Self::Exact => 0,
            Self::Aggregate => 1,
            Self::ReferenceOnly => 2,
            Self::Missing => 3,
        }
    }
    fn from_tag(value: u8) -> Result<Self, PortableError> {
        match value {
            0 => Ok(Self::Exact),
            1 => Ok(Self::Aggregate),
            2 => Ok(Self::ReferenceOnly),
            3 => Ok(Self::Missing),
            _ => Err(PortableError::Invalid),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AvailabilityClaim {
    pub archive: Availability,
    pub current: Availability,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortableError {
    Limit,
    Invalid,
    DigestMismatch,
    Membership,
    View(ViewError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortableView {
    view: AsOfView,
    availability: BTreeMap<RevisionDigest, AvailabilityClaim>,
}
impl PortableView {
    pub fn new(
        view: AsOfView,
        availability: BTreeMap<RevisionDigest, AvailabilityClaim>,
    ) -> Result<Self, PortableError> {
        if !view.revisions().keys().eq(availability.keys()) {
            return Err(PortableError::Membership);
        }
        Ok(Self { view, availability })
    }
    pub fn view(&self) -> &AsOfView {
        &self.view
    }
    pub fn availability(&self) -> &BTreeMap<RevisionDigest, AvailabilityClaim> {
        &self.availability
    }
    /// The digest authenticates bytes only when the caller binds it to a trusted source.
    pub fn export(&self) -> Result<(Vec<u8>, [u8; 32]), PortableError> {
        let mut w = Writer(Vec::new());
        w.push(MAGIC)?;
        w.count(self.view.revisions().len())?;
        for (id, record) in self.view.revisions() {
            w.push(id)?;
            match record {
                Some(record) => {
                    w.push(&[1])?;
                    w.blob(
                        &record
                            .canonical_bytes()
                            .map_err(|_| PortableError::Invalid)?,
                    )?;
                }
                None => w.push(&[0])?,
            }
            let claim = self.availability.get(id).ok_or(PortableError::Membership)?;
            w.push(&[claim.archive.tag(), claim.current.tag()])?;
        }
        w.count(self.view.relations().len())?;
        for (id, record) in self.view.relations() {
            w.push(id)?;
            match record {
                Some(record) => {
                    w.push(&[1])?;
                    w.blob(
                        &record
                            .canonical_bytes()
                            .map_err(|_| PortableError::Invalid)?,
                    )?;
                }
                None => w.push(&[0])?,
            }
        }
        w.count(self.view.groups().len())?;
        for group in self.view.groups().keys() {
            w.push(&group.conclusion)?;
            w.blob(group.attributed_to.as_bytes())?;
            w.count(group.premises.len())?;
            for (premise, edge) in &group.premises {
                w.push(premise)?;
                w.push(edge)?;
            }
        }
        let digest = Sha256::digest(&w.0).into();
        Ok((w.0, digest))
    }
    pub fn import(bytes: &[u8], expected_digest: [u8; 32]) -> Result<Self, PortableError> {
        if bytes.len() > MAX_PORTABLE_BYTES {
            return Err(PortableError::Limit);
        }
        if <[u8; 32]>::from(Sha256::digest(bytes)) != expected_digest {
            return Err(PortableError::DigestMismatch);
        }
        let mut r = Reader::new(bytes);
        r.expect(MAGIC)?;
        let n = r.bounded_count(MAX_VIEW_REVISIONS)?;
        let mut revisions = BTreeSet::new();
        let mut availability = BTreeMap::new();
        let mut corpus = Corpus::default();
        let mut previous = None;
        for _ in 0..n {
            let id = r.digest()?;
            ordered(&mut previous, id)?;
            revisions.insert(id);
            match r.byte()? {
                0 => {}
                1 => {
                    let raw = r.blob(MAX_CANONICAL_BYTES)?;
                    let record = decode_revision(raw)?;
                    if record.digest().map_err(|_| PortableError::Invalid)? != id {
                        return Err(PortableError::DigestMismatch);
                    }
                    corpus.revisions.insert(id, record);
                }
                _ => return Err(PortableError::Invalid),
            }
            availability.insert(
                id,
                AvailabilityClaim {
                    archive: Availability::from_tag(r.byte()?)?,
                    current: Availability::from_tag(r.byte()?)?,
                },
            );
        }
        let n = r.bounded_count(MAX_VIEW_RELATIONS)?;
        let mut relations = BTreeSet::new();
        let mut previous = None;
        for _ in 0..n {
            let id = r.digest()?;
            ordered(&mut previous, id)?;
            relations.insert(id);
            match r.byte()? {
                0 => {}
                1 => {
                    let raw = r.blob(MAX_CANONICAL_BYTES)?;
                    let record = decode_relation(raw)?;
                    if record.digest().map_err(|_| PortableError::Invalid)? != id {
                        return Err(PortableError::DigestMismatch);
                    }
                    corpus.relations.insert(id, record);
                }
                _ => return Err(PortableError::Invalid),
            }
        }
        let n = r.bounded_count(MAX_VIEW_GROUPS)?;
        let mut groups = BTreeSet::new();
        let mut total = 0usize;
        let mut previous_group: Option<SupportGroup> = None;
        for _ in 0..n {
            let conclusion = r.digest()?;
            let attributed_to = r.string(MAX_ATTRIBUTION_BYTES)?;
            let count = r.bounded_count(MAX_GROUP_PREMISES)?;
            total = total.checked_add(count).ok_or(PortableError::Limit)?;
            if total > MAX_TOTAL_PREMISES {
                return Err(PortableError::Limit);
            }
            let mut premises = BTreeMap::new();
            let mut previous = None;
            for _ in 0..count {
                let id = r.digest()?;
                ordered(&mut previous, id)?;
                premises.insert(id, r.digest()?);
            }
            let group = SupportGroup {
                conclusion,
                attributed_to,
                premises,
            };
            if previous_group.as_ref().is_some_and(|prior| prior >= &group) {
                return Err(PortableError::Invalid);
            }
            previous_group = Some(group.clone());
            groups.insert(group);
        }
        if !r.done() {
            return Err(PortableError::Invalid);
        }
        let view = AsOfView::capture(&corpus, revisions, relations, groups)
            .map_err(PortableError::View)?;
        Self::new(view, availability)
    }
}
fn ordered(previous: &mut Option<[u8; 32]>, next: [u8; 32]) -> Result<(), PortableError> {
    if previous.is_some_and(|p| p >= next) {
        return Err(PortableError::Invalid);
    }
    *previous = Some(next);
    Ok(())
}
struct Writer(Vec<u8>);
impl Writer {
    fn push(&mut self, bytes: &[u8]) -> Result<(), PortableError> {
        if self
            .0
            .len()
            .checked_add(bytes.len())
            .is_none_or(|n| n > MAX_PORTABLE_BYTES)
        {
            return Err(PortableError::Limit);
        }
        self.0.extend_from_slice(bytes);
        Ok(())
    }
    fn count(&mut self, n: usize) -> Result<(), PortableError> {
        self.push(&(n as u64).to_be_bytes())
    }
    fn blob(&mut self, bytes: &[u8]) -> Result<(), PortableError> {
        self.count(bytes.len())?;
        self.push(bytes)
    }
}
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    items: usize,
}
impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            items: 0,
        }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], PortableError> {
        let end = self.pos.checked_add(n).ok_or(PortableError::Limit)?;
        let bytes = self.data.get(self.pos..end).ok_or(PortableError::Invalid)?;
        self.pos = end;
        Ok(bytes)
    }
    fn expect(&mut self, value: &[u8]) -> Result<(), PortableError> {
        if self.take(value.len())? == value {
            Ok(())
        } else {
            Err(PortableError::Invalid)
        }
    }
    fn byte(&mut self) -> Result<u8, PortableError> {
        Ok(self.take(1)?[0])
    }
    fn digest(&mut self) -> Result<[u8; 32], PortableError> {
        self.take(32)?
            .try_into()
            .map_err(|_| PortableError::Invalid)
    }
    fn count(&mut self) -> Result<usize, PortableError> {
        let n = u64::from_be_bytes(
            self.take(8)?
                .try_into()
                .map_err(|_| PortableError::Invalid)?,
        );
        usize::try_from(n).map_err(|_| PortableError::Limit)
    }
    fn bounded_count(&mut self, max: usize) -> Result<usize, PortableError> {
        let n = self.count()?;
        if n > max {
            Err(PortableError::Limit)
        } else {
            Ok(n)
        }
    }
    fn item_count(&mut self) -> Result<usize, PortableError> {
        let n = self.bounded_count(MAX_ITEMS)?;
        self.items = self.items.checked_add(n).ok_or(PortableError::Limit)?;
        if self.items > MAX_ITEMS {
            return Err(PortableError::Limit);
        }
        Ok(n)
    }
    fn blob(&mut self, max: usize) -> Result<&'a [u8], PortableError> {
        let n = self.bounded_count(max)?;
        self.take(n)
    }
    fn string(&mut self, max: usize) -> Result<String, PortableError> {
        String::from_utf8(self.blob(max)?.to_vec()).map_err(|_| PortableError::Invalid)
    }
    fn done(&self) -> bool {
        self.pos == self.data.len()
    }
}

fn decode_revision(bytes: &[u8]) -> Result<RevisionBody, PortableError> {
    let mut r = Reader::new(bytes);
    r.expect(REVISION_MAGIC)?;
    let object_id = r.string(MAX_CANONICAL_BYTES)?;
    let kind = match r.byte()? {
        0 => Kind::Hypothesis,
        1 => Kind::Source,
        2 => Kind::Observation,
        3 => Kind::Result,
        4 => Kind::Analysis,
        5 => Kind::Decision,
        6 => Kind::Dissent,
        7 => Kind::Custom {
            namespace: r.string(MAX_CANONICAL_BYTES)?,
            name: r.string(MAX_CANONICAL_BYTES)?,
        },
        _ => return Err(PortableError::Invalid),
    };
    let text = r.string(MAX_CANONICAL_BYTES)?;
    let mut parents = BTreeSet::new();
    let mut previous = None;
    for _ in 0..r.item_count()? {
        let id = r.digest()?;
        ordered(&mut previous, id)?;
        parents.insert(id);
    }
    let mut sources = Vec::new();
    for _ in 0..r.item_count()? {
        sources.push(match r.byte()? {
            0 => SourceClaim::Captured {
                capture_id: r.string(MAX_CANONICAL_BYTES)?,
                artifact_sha256: r.digest()?,
                locator: r.string(MAX_CANONICAL_BYTES)?,
                parser: r.string(MAX_CANONICAL_BYTES)?,
            },
            1 => SourceClaim::ExternalOnly {
                locator: r.string(MAX_CANONICAL_BYTES)?,
                parser: r.string(MAX_CANONICAL_BYTES)?,
            },
            _ => return Err(PortableError::Invalid),
        });
    }
    let extensions = decode_map(&mut r, 0)?;
    let body = RevisionBody {
        object_id,
        kind,
        text,
        parents,
        sources,
        extensions,
    };
    if !r.done() || body.canonical_bytes().map_err(|_| PortableError::Invalid)? != bytes {
        return Err(PortableError::Invalid);
    }
    Ok(body)
}
fn decode_relation(bytes: &[u8]) -> Result<Relation, PortableError> {
    let mut r = Reader::new(bytes);
    r.expect(RELATION_MAGIC)?;
    let subject = r.digest()?;
    let predicate = match r.byte()? {
        0 => Predicate::Supports,
        1 => Predicate::Refines,
        2 => Predicate::Supersedes,
        3 => Predicate::Falsifies,
        4 => Predicate::Waives,
        5 => Predicate::Custom {
            namespace: r.string(MAX_CANONICAL_BYTES)?,
            name: r.string(MAX_CANONICAL_BYTES)?,
        },
        _ => return Err(PortableError::Invalid),
    };
    let object = r.digest()?;
    let attributed_to = r.string(MAX_CANONICAL_BYTES)?;
    let extensions = decode_map(&mut r, 0)?;
    let relation = Relation {
        subject,
        predicate,
        object,
        attributed_to,
        extensions,
    };
    if !r.done()
        || relation
            .canonical_bytes()
            .map_err(|_| PortableError::Invalid)?
            != bytes
    {
        return Err(PortableError::Invalid);
    }
    Ok(relation)
}
fn decode_map(r: &mut Reader<'_>, depth: usize) -> Result<BTreeMap<String, Value>, PortableError> {
    let count = r.item_count()?;
    let mut map = BTreeMap::new();
    let mut previous: Option<String> = None;
    for _ in 0..count {
        let key = r.string(MAX_CANONICAL_BYTES)?;
        if previous.as_ref().is_some_and(|p| p >= &key) {
            return Err(PortableError::Invalid);
        }
        previous = Some(key.clone());
        map.insert(key, decode_value(r, depth)?);
    }
    Ok(map)
}
fn decode_value(r: &mut Reader<'_>, depth: usize) -> Result<Value, PortableError> {
    if depth > MAX_DEPTH {
        return Err(PortableError::Limit);
    }
    match r.byte()? {
        0 => Ok(Value::Null),
        1 => Ok(Value::Bool(false)),
        2 => Ok(Value::Bool(true)),
        3 => Ok(Value::Text(r.string(MAX_CANONICAL_BYTES)?)),
        4 => {
            let count = r.item_count()?;
            let mut values = Vec::with_capacity(count);
            for _ in 0..count {
                values.push(decode_value(r, depth + 1)?);
            }
            Ok(Value::Array(values))
        }
        5 => Ok(Value::Object(decode_map(r, depth + 1)?)),
        _ => Err(PortableError::Invalid),
    }
}
