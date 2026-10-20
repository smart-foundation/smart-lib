use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::schema::{manifest_capnp, types_capnp};

/// One dependency of a manifest — the contract's `Dependency`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ManifestDependency {
    /// The schema the dependency names.
    pub schema_id: u64,
    /// The dependency's human name.
    pub name: String,
    /// Whether the dependency is optional.
    pub optional: bool,
}

/// One manifest — the contract's `Manifest`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ManifestEntry {
    /// The manifest's name — the opaque id the contract addresses it by.
    pub name: String,
    /// The human-readable label.
    pub display_name: String,
    /// The description.
    pub description: String,
    /// The vendor.
    pub vendor: String,
    /// The semantic version (`"1.2.3"`).
    pub version: String,
    /// The entry points.
    pub entries: Vec<String>,
    /// The schemas the manifest depends on.
    pub depends: Vec<ManifestDependency>,
    /// The schemas the manifest publishes.
    pub offers: Vec<String>,
    /// The bundle shape, as an open token.
    pub shape: String,
}

impl ManifestEntry {
    /// The version's numeric parts, as the contract's `T.Version` carries
    /// them. A version that is not three numbers is carried as it was
    /// spelled — the store never invents a number for it.
    pub fn version_parts(&self) -> (u32, u32, u32) {
        let mut parts = self.version.split('.');
        let major = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        let patch = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        (major, minor, patch)
    }

    /// Write into a generated `Manifest` builder.
    pub fn write_to(&self, builder: &mut manifest_capnp::manifest::Builder<'_>) {
        // The contract's `T.ManifestName` is an opaque id; the name this
        // manifest is filed under is addressed by its id, and the human
        // spelling travels beside it as the display name.
        write_manifest_name(&mut builder.reborrow().init_name(), &self.name);
        builder.set_display_name(&self.display_name);
        builder.set_description(&self.description);
        builder.set_vendor(&self.vendor);
        let (major, minor, patch) = self.version_parts();
        {
            let mut version = builder.reborrow().init_version();
            version.set_major(major);
            version.set_minor(minor);
            version.set_patch(patch);
        }
        let mut entries = builder.reborrow().init_entries(self.entries.len() as u32);
        for (index, entry) in self.entries.iter().enumerate() {
            let mut point = entries.reborrow().get(index as u32);
            point.set_name(entry);
            point.set_path(entry);
        }
        let mut depends = builder.reborrow().init_depends(self.depends.len() as u32);
        for (index, dependency) in self.depends.iter().enumerate() {
            let mut entry = depends.reborrow().get(index as u32);
            entry.reborrow().init_schema_id().set_id(dependency.schema_id);
            entry.set_name(&dependency.name);
            entry.set_optional(dependency.optional);
        }
    }

    /// Read from a generated `Manifest` reader.
    pub fn from_reader(reader: &manifest_capnp::manifest::Reader<'_>) -> Self {
        let mut entries = Vec::new();
        if let Ok(list) = reader.get_entries() {
            for entry in list.iter() {
                let path = crate::domains::manifest::types::text_of(entry.get_path());
                let name = crate::domains::manifest::types::text_of(entry.get_name());
                entries.push(if path.is_empty() { name } else { path });
            }
        }
        let mut depends = Vec::new();
        if let Ok(list) = reader.get_depends() {
            for entry in list.iter() {
                depends.push(ManifestDependency {
                    schema_id: entry
                        .get_schema_id()
                        .map(|id| id.get_id())
                        .unwrap_or_default(),
                    name: crate::domains::manifest::types::text_of(entry.get_name()),
                    optional: entry.get_optional(),
                });
            }
        }
        let version = reader
            .get_version()
            .map(|version| {
                alloc::format!(
                    "{}.{}.{}",
                    version.get_major(),
                    version.get_minor(),
                    version.get_patch()
                )
            })
            .unwrap_or_default();
        ManifestEntry {
            // The contract's name is an opaque id; the store keys the manifest
            // by its own name, so the id is what the wire form carries back.
            name: reader
                .get_name()
                .map(|name| name.get_id().to_string())
                .unwrap_or_default(),
            display_name: crate::domains::manifest::types::text_of(reader.get_display_name()),
            description: crate::domains::manifest::types::text_of(reader.get_description()),
            vendor: crate::domains::manifest::types::text_of(reader.get_vendor()),
            version,
            entries,
            depends,
            offers: Vec::new(),
            shape: String::new(),
        }
    }
}

/// The validation answer — the contract's `ValidationResult`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ValidationReport {
    /// Whether the manifest is valid.
    pub valid: bool,
    /// What is wrong with it, when it is not.
    pub errors: Vec<String>,
}

/// The pagination window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pagination {
    /// The first row to return.
    pub offset: u32,
    /// How many rows to return (0 means one — never "everything").
    pub limit: u32,
}

impl Pagination {
    /// The effective limit: a zero limit means one row, never an unbounded
    /// read.
    pub const fn effective_limit(self) -> u32 {
        if self.limit == 0 {
            1
        } else {
            self.limit
        }
    }
}

/// A page of rows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page<T> {
    /// The rows.
    pub items: Vec<T>,
    /// How many rows the store holds in total.
    pub total: u64,
}

/// A subscription handle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SubscriptionId(pub u64);

/// Read a capnp text field into an owned string.
pub(crate) fn text_of(reader: capnp::Result<capnp::text::Reader<'_>>) -> String {
    match reader {
        Ok(text) => core::str::from_utf8(text.as_bytes()).unwrap_or("").to_string(),
        Err(_) => String::new(),
    }
}

/// The `T.ManifestName` of a manifest — its opaque id.
pub(crate) fn manifest_name_id(name: &str) -> u64 {
    // A stable id for the name: the FNV-1a of the name's bytes. It is an
    // address, not an identity claim — the contract's own `T.ManifestName` is
    // an opaque id, and the name it stands for travels beside it.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in name.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The `T.ManifestName` builder.
pub(crate) fn write_manifest_name(
    builder: &mut types_capnp::manifest_name::Builder<'_>,
    name: &str,
) {
    builder.set_id(manifest_name_id(name));
}
