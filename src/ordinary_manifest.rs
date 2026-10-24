//! Actual ordinary local payload histories; physical locators are separate from content identities.
use std::fs;
use anyhow::{Context,Result};
use camino::{Utf8Path,Utf8PathBuf};
use serde::{Serialize,Deserialize};
use crate::{catalogue::{Catalogue,Format},cas_remote::{Placement,Style},snapshot_format::{FlatDirectory,FlatSnapshot}};
const SCHEMA:&str="projcli.ordinary-local-snapshot.v1";

/// Explicit ordinary metadata policy; absent inline policies preserve legacy behavior.
#[derive(Clone)]
pub(crate) struct Policy {placement:Placement,format:Format}
impl Policy {
    pub(crate) fn parse(definition:&crate::restore::Definition)->Result<Option<Self>> {
        if definition.manifest.is_none()&&definition.manifest_style.is_none()&&definition.manifest_format.is_none(){return Ok(None)}
        anyhow::ensure!(definition.manifest_style.unwrap_or(Style::Flat)==Style::Flat,
            "Ordinary tree manifests are not implemented yet; use manifest-style: flat");
        Ok(Some(Self {placement:definition.manifest.unwrap_or(Placement::Auto),
            format:definition.manifest_format.unwrap_or(Format::Csv)}))
    }
    /// Resolve only new metadata; legacy root CSV and VERSION remain compatibility metadata.
    pub(crate) fn locations(&self,root:&Utf8Path,base:&Utf8Path,title:&str)->Result<Locations> {
        let title=icu_normalizer::ComposingNormalizer::new_nfc().normalize(&title.to_uppercase()).into_owned();
        let mut hash=blake3::Hasher::new_derive_key("projcli.ordinary-remote-name.v1");
        hash.update(&serde_json::to_vec(&("local",title))?);let key=hash.finalize().to_hex().to_string();
        let project=root.join(".proj/remotes").join(&key[..2]).join(&key[2..]);
        let remote=base.join(".projcli-manifests");
        let bases=match self.placement {
            Placement::Project=>vec![project.clone()],Placement::Remote=>vec![remote],
            Placement::Auto|Placement::Both=>vec![project.clone(),remote],
        };
        let result=Locations {bases,project,format:self.format};result.preflight(root)?;Ok(result)
    }
}
/// Selected complete history locations and the always-small project binding.
#[derive(Clone)]
pub(crate) struct Locations {bases:Vec<Utf8PathBuf>,project:Utf8PathBuf,format:Format}
/// One actual payload location, relative to the configured remote base.
#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Payload {
    pub(crate) label:String,
    pub(crate) inventory_hash:String,
    pub(crate) path:String,
    pub(crate) empty:bool,
    /// Verbatim legacy VERSION entry; an asterisk still means source agreement is untrusted.
    pub(crate) advertised_version:Option<String>,
}
#[derive(Serialize)]
struct Binding<'a> {schema:&'static str,hash_algorithm:&'static str,version:&'a str,layout:&'static str,payloads:&'a [Payload]}
impl Locations {
    pub(crate) fn preflight(&self,root:&Utf8Path)->Result<()> {
        for base in self.bases.iter().chain(std::iter::once(&self.project)) {
            let anchor=base.ancestors().last().context("Ordinary metadata has no filesystem anchor")?;
            crate::path_safety::validate_native_names(anchor,base.strip_prefix(anchor)?.as_str(),false)?;
            crate::cas_tree::project_target(root,base,true)?;
            for relative in ["catalogues/v1","snapshots/v1","references/v1"] {
                crate::cas::checked_directory(&crate::cas_tree::native(base,relative)?)?;
            }
            for format in [Format::Csv,Format::Json,Format::CsvDedupeFile,Format::JsonDedupeFile] {
                let path=crate::cas_tree::native(base,crate::cas_flat::filename(format))?;
                if crate::yml::configuration_entry_exists(&path)? {
                    anyhow::ensure!(fs::symlink_metadata(&path)?.is_file(),"Ordinary catalogue must be regular");
                }
            }
        }
        self.history()?;Ok(())
    }
    pub(crate) fn paths(&self)->impl Iterator<Item=&Utf8Path> {
        self.bases.iter().chain(std::iter::once(&self.project)).map(Utf8PathBuf::as_path)
    }
    pub(crate) fn history(&self)->Result<Catalogue> {crate::cas_flat::load_history(&self.bases)}
    /// Reject conflicting retries against final private-stage membership, before installation.
    pub(crate) fn check(&self,history:&Catalogue,version:&str,directory:FlatDirectory)->Result<()> {
        crate::cas_flat::append_idempotent(history,&FlatSnapshot::new(version.into(),None,vec![directory])?)?.encode(self.format)?;
        Ok(())
    }
    /// Write complete catalogue copies, then immutable full bindings, then project reference.
    /// Earlier metadata may remain if another copy fails; never claim a remote transaction.
    pub(crate) fn publish(&self,root:&Utf8Path,history:&Catalogue,version:&str,archive:bool,
        directories:Vec<FlatDirectory>,mut payloads:Vec<Payload>)->Result<()> {
        self.preflight(root)?;
        let snapshot=FlatSnapshot::new(version.into(),None,directories)?;
        let updated=crate::cas_flat::append_idempotent(history,&snapshot)?;
        payloads.sort_by(|a,b|a.label.as_bytes().cmp(b.label.as_bytes()));
        for payload in &mut payloads {
            payload.inventory_hash=updated.records.iter().find(|row|row.label==payload.label&&row.version==version)
                .context("Published inventory is missing")?.inventory_hash.clone();
        }
        let binding=Binding {schema:SCHEMA,hash_algorithm:"blake3",version,
            layout:if archive{"archive"}else{"latest"},payloads:&payloads};
        let canonical=serde_json::to_vec(&binding)?;
        let mut hash=blake3::Hasher::new_derive_key(SCHEMA);hash.update(&canonical);let id=hash.finalize().to_hex().to_string();
        let binding=serde_json::to_vec_pretty(&binding)?;
        anyhow::ensure!(binding.len()<=crate::snapshot_format::MAX_METADATA_BYTES,"Ordinary binding exceeds metadata limit");
        let bytes=updated.encode(self.format)?;
        for base in &self.bases {
            let path=crate::cas_tree::native(base,crate::cas_flat::filename(self.format))?;
            if crate::yml::configuration_entry_exists(&path)?
                &&crate::fs_utils::read_managed_bounded(&path,crate::snapshot_format::MAX_METADATA_BYTES as u64)?==bytes {continue}
            crate::cas::write_metadata(&path,&bytes,false)?;
        }
        for base in &self.bases {write_binding(base,"snapshots/v1",&id,&binding)?;}
        write_binding(&self.project,"references/v1",&id,&binding)?;
        Ok(())
    }
}
fn write_binding(base:&Utf8Path,namespace:&str,id:&str,bytes:&[u8])->Result<()> {
    let path=crate::cas_tree::native(base,&format!("{namespace}/{}/{}.json",&id[..2],&id[2..]))?;
    crate::cas_tree::publish_metadata(&path,bytes,|existing|{
        anyhow::ensure!(existing==bytes,"Existing ordinary snapshot binding conflicts; preserved");Ok(())
    })
}
/// Observe final staged/published bytes, not the project's source manifest.
/// Ordinary empty markers intentionally discard file-empty directory shapes.
pub(crate) fn observe(path:&Utf8Path,label:&str)->Result<FlatDirectory> {
    let mut directory=crate::cas_tree::observe_directory(path,label)?;
    if directory.files.is_empty(){directory.empty_directories.clear();}
    Ok(directory)
}
