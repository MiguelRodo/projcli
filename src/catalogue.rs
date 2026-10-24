//! Bounded, self-contained flat catalogue codecs. No active config or store migration.
//! Version references deduplicate a complete logical artefact; file dictionaries
//! preserve every path membership. Neither mechanism changes raw file CAS identity.
//!
//! ```
//! use proj::catalogue::{Catalogue, Format};
//! let catalogue=Catalogue::default();
//! let csv=catalogue.encode(Format::Csv)?;
//! assert_eq!(Catalogue::parse(&csv,Format::Csv)?,catalogue);
//! # Ok::<(),anyhow::Error>(())
//! ```
use std::collections::{BTreeMap,HashMap,HashSet};
use anyhow::{Context,Result};
use serde::{Serialize,Deserialize};
use crate::snapshot_format::{FlatSnapshot,FlatDirectory,FileRecord,MAX_METADATA_BYTES,MAX_RECORDS};

const SCHEMA:&str="projcli.flat-catalogue.v1";
const ALGORITHM:&str="blake3";

/// Serialization plus optional file-dictionary encoding; independent of flat/tree style.
#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize,Deserialize)]
#[serde(rename_all="kebab-case")]
pub enum Format {
    /// Rectangular CSV with direct complete-inventory version references.
    Csv,
    /// Readable JSON with direct complete-inventory version references.
    Json,
    /// CSV with a bundled hash dictionary and every current file path retained.
    CsvDedupeFile,
    /// JSON with a bundled hash dictionary and every current file path retained.
    JsonDedupeFile,
}
impl Format {
    fn dictionary(self)->bool {matches!(self,Self::CsvDedupeFile|Self::JsonDedupeFile)}
    fn csv(self)->bool {matches!(self,Self::Csv|Self::CsvDedupeFile)}
    fn name(self)->&'static str {match self {
        Self::Csv=>"csv",Self::Json=>"json",Self::CsvDedupeFile=>"csv-dedupe-file",Self::JsonDedupeFile=>"json-dedupe-file"}}
}

/// A complete inventory or a direct reference to an earlier complete same-label version.
#[derive(Clone,Debug,PartialEq,Eq,Serialize,Deserialize)]
#[serde(tag="kind",rename_all="snake_case",deny_unknown_fields)]
pub enum Inventory {
    /// Complete path/hash membership, including explicit empty-directory leaves.
    Complete {
        /// Every file's portable path and full content hash.
        files:Vec<FileRecord>,
        /// Empty directory leaves; empty lists represent an empty logical root.
        empty_directories:Vec<String>,
    },
    /// No file rows are mixed into a version reference.
    Reference {
        /// Earlier complete version for this same logical label; never another reference.
        ref_version:String,
    },
}

/// One immutable logical artefact/version record, separate from physical source paths.
#[derive(Clone,Debug,PartialEq,Eq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtefactVersion {
    /// Exact logical directory label.
    pub label:String,
    /// Supplied build/release version.
    pub version:String,
    /// Optional supplied timestamp, normalized to UTC; not part of inventory equality.
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub created_at:Option<String>,
    /// Domain-separated full digest of sorted names/kinds/content hashes.
    pub inventory_hash:String,
    /// Complete membership or direct same-artefact version reference.
    pub inventory:Inventory,
}

/// Append-ordered self-contained catalogue; references target preceding complete records.
/// Append order is explicit history, not a guess based on lexical version sorting.
#[derive(Clone,Debug,Default,PartialEq,Eq)]
pub struct Catalogue {
    /// Records in append order. Mutated values are validated before encoding or lookup.
    pub records:Vec<ArtefactVersion>,
}

fn bounded(bytes:Vec<u8>)->Result<Vec<u8>> {
    anyhow::ensure!(bytes.len()<=MAX_METADATA_BYTES,"Catalogue exceeds metadata byte limit");Ok(bytes)
}
fn canonical_directory(directory:FlatDirectory)->Result<FlatDirectory> {
    Ok(FlatSnapshot::new("inventory".into(),None,vec![directory])?.directories.remove(0))
}
fn inventory_id(directory:&FlatDirectory)->Result<String> {
    #[derive(Serialize)]
    struct Content<'a> {files:&'a [FileRecord],empty_directories:&'a [String]}
    let bytes=bounded(serde_json::to_vec(&Content {files:&directory.files,empty_directories:&directory.empty_directories})?)?;
    let mut hash=blake3::Hasher::new_derive_key("projcli.artefact-inventory.v1");hash.update(&bytes);
    Ok(hash.finalize().to_hex().to_string())
}
fn text_budget(total:&mut usize,text:&str)->Result<()> {
    *total=total.checked_add(text.len()).context("Catalogue text budget overflow")?;
    anyhow::ensure!(*total<=8*1024*1024,"Catalogue exceeds text budget");Ok(())
}
impl Catalogue {
    /// Validate references, hashes, portable names, uniqueness and explicit resource limits.
    /// Complete targets must precede references and be for the same exact logical label.
    pub fn validate(&self)->Result<()> {
        anyhow::ensure!(self.records.len()<=MAX_RECORDS,"Catalogue exceeds record limit");
        let mut rows=0usize;let mut text=0usize;let mut labels=HashMap::<String,String>::new();
        let mut previous=HashMap::<(&str,&str),&ArtefactVersion>::new();
        for record in &self.records {
            rows=rows.checked_add(1).context("Catalogue row budget overflow")?;
            for value in [&record.label,&record.version,&record.inventory_hash] {text_budget(&mut text,value)?;}
            if let Some(date)=&record.created_at {text_budget(&mut text,date)?;}
            if let Inventory::Complete {files,empty_directories}=&record.inventory {
                rows=rows.checked_add(files.len()).and_then(|n|n.checked_add(empty_directories.len()))
                    .context("Catalogue row budget overflow")?;
                for file in files {text_budget(&mut text,&file.path)?;text_budget(&mut text,&file.hash)?;}
                for path in empty_directories {text_budget(&mut text,path)?;}
            }else if let Inventory::Reference {ref_version}=&record.inventory {text_budget(&mut text,ref_version)?;}
            anyhow::ensure!(rows<=MAX_RECORDS,"Catalogue exceeds expanded record limit");
            let metadata=FlatSnapshot::new(record.version.clone(),record.created_at.clone(),
                vec![FlatDirectory {label:record.label.clone(),files:vec![],empty_directories:vec![]}])?;
            anyhow::ensure!(metadata.created_at==record.created_at,"Catalogue date is not normalized UTC");
            crate::cas::validate_digest(&record.inventory_hash)?;
            let key=crate::path_safety::portable_path_key(camino::Utf8Path::new(&record.label)).to_string();
            if let Some(label)=labels.insert(key,record.label.clone()) {
                anyhow::ensure!(label==record.label,"Portable-alias logical labels in catalogue");
            }
            anyhow::ensure!(!previous.contains_key(&(record.label.as_str(),record.version.as_str())),
                "Duplicate artefact/version record");
            match &record.inventory {
                Inventory::Complete {files,empty_directories}=>{
                    let directory=canonical_directory(FlatDirectory {label:record.label.clone(),
                        files:files.clone(),empty_directories:empty_directories.clone()})?;
                    anyhow::ensure!(inventory_id(&directory)?==record.inventory_hash,"Corrupt complete inventory digest");
                },
                Inventory::Reference {ref_version}=>{
                    let target=previous.get(&(record.label.as_str(),ref_version.as_str()))
                        .context("Dangling, forward or cross-label inventory reference")?;
                    anyhow::ensure!(matches!(&target.inventory,Inventory::Complete {..}),
                        "Inventory reference chains are unsupported");
                    anyhow::ensure!(target.inventory_hash==record.inventory_hash,"Inventory reference digest mismatch");
                },
            }
            previous.insert((&record.label,&record.version),record);
        }
        Ok(())
    }
    /// Append selected complete observations. Equal inventories point directly to the
    /// earliest complete same-label record, including when reverting to older bytes.
    /// Existing records are preserved; an existing label/version is never overwritten.
    pub fn with_snapshot(&self,snapshot:&FlatSnapshot)->Result<Self> {
        self.validate()?;
        let snapshot=FlatSnapshot::parse(&snapshot.readable_json()?)?;
        let mut result=self.clone();
        for directory in snapshot.directories {
            let hash=inventory_id(&directory)?;
            let target=self.records.iter().find(|r|r.label==directory.label&&r.inventory_hash==hash
                &&matches!(&r.inventory,Inventory::Complete {..}));
            let inventory=if let Some(target)=target {Inventory::Reference {ref_version:target.version.clone()}}
                else {Inventory::Complete {files:directory.files,empty_directories:directory.empty_directories}};
            result.records.push(ArtefactVersion {label:directory.label,version:snapshot.version.clone(),
                created_at:snapshot.created_at.clone(),inventory_hash:hash,inventory});
        }
        result.validate()?;Ok(result)
    }
    /// Resolve one exact logical label/version to complete membership without filesystem I/O.
    pub fn expanded(&self,label:&str,version:&str)->Result<FlatDirectory> {
        self.validate()?;
        let record=self.records.iter().find(|r|r.label==label&&r.version==version)
            .context("Requested artefact/version is absent")?;
        let target=match &record.inventory {
            Inventory::Reference {ref_version}=>self.records.iter().find(|r|r.label==label&&r.version==*ref_version)
                .context("Referenced complete inventory is absent")?,
            Inventory::Complete {..}=>record,
        };
        if let Inventory::Complete {files,empty_directories}=&target.inventory {
            canonical_directory(FlatDirectory {label:label.into(),files:files.clone(),empty_directories:empty_directories.clone()})
        }else{anyhow::bail!("Inventory reference is not complete")}
    }
    /// Encode CSV/JSON with version references and, only when requested, a bundled file dictionary.
    /// CSV index columns preserve history order even after Excel sorts rows.
    pub fn encode(&self,format:Format)->Result<Vec<u8>> {
        let wire=Wire::from_catalogue(self,format)?;
        if format.csv(){wire.csv()}else{
            let mut bytes=serde_json::to_vec_pretty(&wire)?;bytes.push(b'\n');bounded(bytes)
        }
    }
    /// Parse an explicit v1 CSV/JSON catalogue, validating all dictionary and inventory references.
    /// Existing projr CSV and legacy flat CAS readers remain separate and unchanged.
    pub fn parse(bytes:&[u8],format:Format)->Result<Self> {
        anyhow::ensure!(bytes.len()<=MAX_METADATA_BYTES,"Catalogue exceeds metadata byte limit");
        let wire:Wire=if format.csv(){Wire::from_csv(bytes)?}else{serde_json::from_slice(bytes).context("Invalid catalogue JSON")?};
        anyhow::ensure!(wire.format==format,"Catalogue serialization/dedupe mode mismatch");
        wire.catalogue()
    }
}

#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    schema:String,format:Format,hash_algorithm:String,
    #[serde(default,skip_serializing_if="Vec::is_empty")]
    dictionary:Vec<Dictionary>,
    records:Vec<WireRecord>,
}
#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct Dictionary {file_id:u32,hash:String}
#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct WireRecord {
    label:String,version:String,
    #[serde(default,skip_serializing_if="Option::is_none")]
    created_at:Option<String>,
    inventory_hash:String,inventory:WireInventory,
}
#[derive(Serialize,Deserialize)]
#[serde(tag="kind",rename_all="snake_case",deny_unknown_fields)]
enum WireInventory {
    Complete {files:Vec<WireFile>,empty_directories:Vec<String>},
    Reference {ref_version:String},
}
#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct WireFile {
    path:String,
    #[serde(default,skip_serializing_if="Option::is_none")]
    hash:Option<String>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    file_id:Option<u32>,
}
const HEADER:[&str;13]=["schema","format","hash_algorithm","record","record_index","label","version",
    "created_at","inventory_hash","path","hash","ref_version","file_id"];
impl Wire {
    fn from_catalogue(value:&Catalogue,format:Format)->Result<Self> {
        value.validate()?;
        let mut hashes=std::collections::BTreeSet::new();
        if format.dictionary() {
            for record in &value.records {
                if let Inventory::Complete {files,..}=&record.inventory {for file in files {hashes.insert(file.hash.clone());}}
            }
        }
        let ids:HashMap<_,_>=hashes.into_iter().enumerate().map(|(i,hash)|(hash,(i+1) as u32)).collect();
        let mut dictionary:Vec<_>=ids.iter().map(|(hash,id)|Dictionary {file_id:*id,hash:hash.clone()}).collect();
        dictionary.sort_by_key(|entry|entry.file_id);
        let mut records=Vec::new();
        for record in &value.records {
            let inventory=match &record.inventory {
                Inventory::Reference {ref_version}=>WireInventory::Reference {ref_version:ref_version.clone()},
                Inventory::Complete {files,empty_directories}=>{
                    let expanded=canonical_directory(FlatDirectory {label:record.label.clone(),
                        files:files.clone(),empty_directories:empty_directories.clone()})?;
                    WireInventory::Complete {files:expanded.files.into_iter().map(|file|WireFile {
                        file_id:ids.get(&file.hash).copied(),hash:if format.dictionary(){None}else{Some(file.hash)},path:file.path
                    }).collect(),empty_directories:expanded.empty_directories}
                },
            };
            records.push(WireRecord {label:record.label.clone(),version:record.version.clone(),
                created_at:record.created_at.clone(),inventory_hash:record.inventory_hash.clone(),inventory});
        }
        let rows=records.iter().try_fold(1+dictionary.len(),|sum,record| {
            let count=match &record.inventory {
                WireInventory::Complete {files,empty_directories}=>1+files.len()+empty_directories.len(),
                WireInventory::Reference {..}=>1,
            };
            sum.checked_add(count).context("Catalogue wire-row budget overflow")
        })?;
        anyhow::ensure!(rows<=MAX_RECORDS,"Catalogue exceeds wire-row limit");
        Ok(Self {schema:SCHEMA.into(),format,hash_algorithm:ALGORITHM.into(),dictionary,records})
    }
    fn catalogue(self)->Result<Catalogue> {
        anyhow::ensure!(self.schema==SCHEMA&&self.hash_algorithm==ALGORITHM,"Unsupported catalogue schema/hash algorithm");
        anyhow::ensure!(self.records.len()+self.dictionary.len()<=MAX_RECORDS,"Catalogue exceeds record limit");
        anyhow::ensure!(self.format.dictionary()||self.dictionary.is_empty(),"Unexpected file dictionary in ordinary mode");
        let mut dictionary=HashMap::new();let mut hashes=HashSet::new();let mut used=HashSet::new();
        for entry in self.dictionary {
            crate::cas::validate_digest(&entry.hash)?;
            anyhow::ensure!(entry.file_id>0&&dictionary.insert(entry.file_id,entry.hash.clone()).is_none()
                &&hashes.insert(entry.hash),"Duplicate/invalid dictionary ID or digest");
        }
        let mut records=Vec::new();
        for record in self.records {
            let inventory=match record.inventory {
                WireInventory::Reference {ref_version}=>Inventory::Reference {ref_version},
                WireInventory::Complete {files,empty_directories}=>{
                    let mut resolved=Vec::new();
                    for file in files {
                        let hash=if self.format.dictionary() {
                            anyhow::ensure!(file.hash.is_none(),"Dictionary membership cannot contain a direct hash");
                            let id=file.file_id.context("Dictionary membership is missing file_id")?;
                            used.insert(id);dictionary.get(&id).context("Dangling file dictionary ID")?.clone()
                        }else{
                            anyhow::ensure!(file.file_id.is_none(),"Ordinary membership cannot contain file_id");
                            file.hash.context("Ordinary membership is missing full hash")?
                        };
                        resolved.push(FileRecord {path:file.path,hash});
                    }
                    let canonical=canonical_directory(FlatDirectory {label:record.label.clone(),files:resolved,empty_directories})?;
                    Inventory::Complete {files:canonical.files,empty_directories:canonical.empty_directories}
                },
            };
            records.push(ArtefactVersion {label:record.label,version:record.version,created_at:record.created_at,
                inventory_hash:record.inventory_hash,inventory});
        }
        anyhow::ensure!(used.len()==dictionary.len(),"Unused file dictionary entries are unsupported");
        let result=Catalogue {records};result.validate()?;Ok(result)
    }
    fn csv(&self)->Result<Vec<u8>> {
        let mut writer=csv::WriterBuilder::new().terminator(csv::Terminator::CRLF).from_writer(Vec::new());
        writer.write_record(HEADER)?;
        let base=||vec![self.schema.clone(),self.format.name().into(),self.hash_algorithm.clone(),
            String::new(),String::new(),String::new(),String::new(),String::new(),String::new(),
            String::new(),String::new(),String::new(),String::new()];
        let mut metadata=base();metadata[3]="catalogue".into();writer.write_record(metadata)?;
        for entry in &self.dictionary {
            let mut row=base();row[3]="dictionary".into();row[10]=entry.hash.clone();row[12]=entry.file_id.to_string();
            writer.write_record(row)?;
        }
        for (index,record) in self.records.iter().enumerate() {
            let mut common=base();common[4]=index.to_string();common[5]=record.label.clone();common[6]=record.version.clone();
            common[7]=record.created_at.clone().unwrap_or_default();common[8]=record.inventory_hash.clone();
            match &record.inventory {
                WireInventory::Reference {ref_version}=>{
                    common[3]="reference".into();common[11]=ref_version.clone();writer.write_record(common)?;
                },
                WireInventory::Complete {files,empty_directories}=>{
                    common[3]="inventory".into();writer.write_record(&common)?;
                    for file in files {
                        let mut row=common.clone();row[3]="file".into();row[9]=file.path.clone();
                        row[10]=file.hash.clone().unwrap_or_default();row[12]=file.file_id.map(|id|id.to_string()).unwrap_or_default();
                        writer.write_record(row)?;
                    }
                    for path in empty_directories {
                        let mut row=common.clone();row[3]="empty_directory".into();row[9]=path.clone();writer.write_record(row)?;
                    }
                },
            }
        }
        writer.flush()?;bounded(writer.into_inner().map_err(|error|error.into_error())?)
    }
    fn from_csv(bytes:&[u8])->Result<Self> {
        let mut reader=csv::ReaderBuilder::new().from_reader(bytes);
        anyhow::ensure!(reader.headers()?.iter().eq(HEADER),"Unsupported rectangular catalogue CSV header");
        let mut format=None;let mut dictionary=Vec::new();let mut catalogue_row=false;
        let mut groups=BTreeMap::<usize,(WireRecord,bool)>::new();let mut row_count=0usize;
        for row in reader.records() {
            row_count+=1;anyhow::ensure!(row_count<=MAX_RECORDS,"Catalogue exceeds CSV row limit");
            let row=row?;anyhow::ensure!(row.len()==HEADER.len(),"Invalid catalogue row width");
            anyhow::ensure!(&row[0]==SCHEMA&&&row[2]==ALGORITHM,"Unsupported catalogue schema/hash algorithm");
            let selected=match &row[1] {"csv"=>Format::Csv,"csv-dedupe-file"=>Format::CsvDedupeFile,
                _=>anyhow::bail!("Invalid CSV catalogue mode")};
            if let Some(old)=format {anyhow::ensure!(old==selected,"Mixed catalogue modes");}else{format=Some(selected)}
            let empty=|indices:&[usize]|->Result<()> {
                anyhow::ensure!(indices.iter().all(|&i|row[i].is_empty()),"Unexpected nonempty fields for CSV record kind");Ok(())
            };
            if &row[3]=="catalogue" {
                empty(&[4,5,6,7,8,9,10,11,12])?;
                anyhow::ensure!(!catalogue_row,"Duplicate catalogue metadata row");catalogue_row=true;continue;
            }
            if &row[3]=="dictionary" {
                empty(&[4,5,6,7,8,9,11])?;
                dictionary.push(Dictionary {file_id:row[12].parse().context("Invalid dictionary ID")?,hash:row[10].into()});
                continue;
            }
            let index:usize=row[4].parse().context("Invalid catalogue record index")?;
            anyhow::ensure!(index<MAX_RECORDS,"Catalogue record index is too large");
            let new_record=||WireRecord {label:row[5].into(),version:row[6].into(),
                created_at:if row[7].is_empty(){None}else{Some(row[7].into())},
                inventory_hash:row[8].into(),inventory:WireInventory::Complete {files:vec![],empty_directories:vec![]}};
            let (record,primary)=groups.entry(index).or_insert_with(||(new_record(),false));
            anyhow::ensure!(record.label==row[5]&&record.version==row[6]&&record.created_at==new_record().created_at
                &&record.inventory_hash==row[8],"Conflicting repeated CSV record metadata");
            match &row[3] {
                "inventory"=>{empty(&[9,10,11,12])?;anyhow::ensure!(!*primary,"Duplicate primary catalogue row");*primary=true;},
                "reference"=>{
                    empty(&[9,10,12])?;anyhow::ensure!(!*primary,"Duplicate primary catalogue row");*primary=true;
                    if let WireInventory::Complete {files,empty_directories}=&record.inventory {
                        anyhow::ensure!(files.is_empty()&&empty_directories.is_empty(),"Reference mixed with membership rows");
                    }
                    record.inventory=WireInventory::Reference {ref_version:row[11].into()};
                },
                "file"|"empty_directory"=>{
                    empty(&[11])?;
                    if let WireInventory::Complete {files,empty_directories}=&mut record.inventory {
                        if &row[3]=="file" {
                            files.push(WireFile {path:row[9].into(),hash:if row[10].is_empty(){None}else{Some(row[10].into())},
                                file_id:if row[12].is_empty(){None}else{Some(row[12].parse().context("Invalid membership ID")?)}});
                        }else{empty(&[10,12])?;empty_directories.push(row[9].into());}
                    }else{anyhow::bail!("Reference mixed with membership rows")}
                },
                _=>anyhow::bail!("Unknown catalogue CSV record kind"),
            }
        }
        let format=format.context("CSV catalogue has no mode-bearing records")?;
        anyhow::ensure!(catalogue_row,"CSV catalogue metadata row is missing");
        let mut records=Vec::new();
        for (expected,(index,(record,primary))) in groups.into_iter().enumerate() {
            anyhow::ensure!(expected==index&&primary,"Missing CSV inventory/reference index row");records.push(record);
        }
        Ok(Self {schema:SCHEMA.into(),hash_algorithm:ALGORITHM.into(),format,dictionary,records})
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    const MODES:[Format;4]=[Format::Csv,Format::Json,Format::CsvDedupeFile,Format::JsonDedupeFile];
    fn directory()->FlatDirectory {
        FlatDirectory {label:"raw-data".into(),files:vec![
            FileRecord {path:"nested/b.txt".into(),hash:blake3::hash(b"beta").to_hex().to_string()},
            FileRecord {path:"a,\"quoted\"-\u{e9}.txt".into(),hash:blake3::hash(b"alpha").to_hex().to_string()},
        ],empty_directories:vec!["empty".into()]}
    }
    fn snapshot(version:&str,directory:FlatDirectory)->FlatSnapshot {
        FlatSnapshot::new(version.into(),Some("2026-10-24T10:00:00+02:00".into()),vec![directory]).unwrap()
    }
    fn history()->Catalogue {
        let first=Catalogue::default().with_snapshot(&snapshot("v1",directory())).unwrap();
        let second=first.with_snapshot(&snapshot("v2",directory())).unwrap();
        let mut changed=directory();changed.files[0].hash=blake3::hash(b"changed").to_hex().to_string();
        let third=second.with_snapshot(&snapshot("v3",changed)).unwrap();
        third.with_snapshot(&snapshot("v4",directory())).unwrap()
    }
    #[test]
    fn all_modes_keep_complete_membership_and_direct_reversion_references() {
        let catalogue=history();
        for index in [1,3] {
            assert_eq!(catalogue.records[index].inventory,Inventory::Reference {ref_version:"v1".into()});
            assert_eq!(catalogue.expanded("raw-data",&format!("v{}",index+1)).unwrap(),directory_canonical());
        }
        for mode in MODES {
            let encoded=catalogue.encode(mode).unwrap();
            assert_eq!(Catalogue::parse(&encoded,mode).unwrap(),catalogue);
            assert_eq!(catalogue.encode(mode).unwrap(),encoded);
        }
        assert!(catalogue.with_snapshot(&snapshot("v1",directory())).is_err());
        assert_eq!(catalogue.records.len(),4);
    }
    fn directory_canonical()->FlatDirectory {canonical_directory(directory()).unwrap()}
    #[test]
    fn equality_ignores_order_date_version_and_physical_root_but_not_membership() {
        let first=Catalogue::default().with_snapshot(&snapshot("v1",directory())).unwrap();
        let mut same=snapshot("v2",directory());same.directories[0].files.reverse();same.created_at=None;
        assert!(matches!(first.with_snapshot(&same).unwrap().records[1].inventory,Inventory::Reference {..}));
        for action in 0..4 {
            let mut changed=directory();
            match action {
                0=>changed.files[0].hash="f".repeat(64),
                1=>changed.files[0].path="renamed.txt".into(),
                2=>{changed.files.pop();},
                _=>changed.files.push(FileRecord {path:"added".into(),hash:"e".repeat(64)}),
            }
            assert!(matches!(first.with_snapshot(&snapshot("v2",changed)).unwrap().records[1].inventory,Inventory::Complete {..}));
        }
        let mut different_label=directory();different_label.label="output".into();
        assert!(matches!(first.with_snapshot(&snapshot("v2",different_label)).unwrap().records[1].inventory,Inventory::Complete {..}));
    }
    #[test]
    fn references_reject_forward_dangling_cross_label_chains_and_corrupt_hashes() {
        let original=history();
        let mut bad=original.clone();bad.records[1].inventory=Inventory::Reference {ref_version:"missing".into()};
        assert!(bad.validate().is_err());
        let mut bad=original.clone();bad.records.swap(0,1);assert!(bad.validate().is_err());
        let mut bad=original.clone();bad.records[3].inventory=Inventory::Reference {ref_version:"v2".into()};
        assert!(bad.validate().is_err());
        let mut bad=original.clone();bad.records[1].label="output".into();assert!(bad.validate().is_err());
        let mut bad=original.clone();bad.records[1].inventory_hash="f".repeat(64);assert!(bad.validate().is_err());
        let mut bad=original.clone();bad.records[0].inventory_hash="f".repeat(64);assert!(bad.validate().is_err());
        let mut bad=original.clone();bad.records.push(bad.records[0].clone());assert!(bad.validate().is_err());
    }
    #[test]
    fn empty_catalogues_and_empty_roots_are_explicit_and_roundtrip_in_all_modes() {
        let empty=Catalogue::default();
        let root=FlatDirectory {label:"output".into(),files:vec![],empty_directories:vec![]};
        let first=empty.with_snapshot(&snapshot("v1",root.clone())).unwrap();
        let later=first.with_snapshot(&snapshot("v2",root.clone())).unwrap();
        assert_eq!(later.expanded("output","v2").unwrap(),root);
        assert!(matches!(later.records[1].inventory,Inventory::Reference {..}));
        for mode in MODES {
            for catalogue in [&empty,&first,&later] {
                assert_eq!(Catalogue::parse(&catalogue.encode(mode).unwrap(),mode).unwrap(),*catalogue);
            }
        }
    }
    #[test]
    fn dictionary_preserves_paths_and_detects_missing_duplicate_wrong_or_unused_entries() {
        let mut files=directory();files.files[0].hash=files.files[1].hash.clone();
        let value=Catalogue::default().with_snapshot(&snapshot("v1",files.clone())).unwrap();
        let encoded=value.encode(Format::JsonDedupeFile).unwrap();
        let wire:serde_json::Value=serde_json::from_slice(&encoded).unwrap();
        assert_eq!(wire["dictionary"].as_array().unwrap().len(),1);
        assert_eq!(wire["records"][0]["inventory"]["files"].as_array().unwrap().len(),2);
        let corrupt=|wire:&serde_json::Value|Catalogue::parse(&serde_json::to_vec(wire).unwrap(),Format::JsonDedupeFile).is_err();
        let mut bad=wire.clone();bad["dictionary"]=serde_json::json!([]);assert!(corrupt(&bad));
        let mut bad=wire.clone();let extra=bad["dictionary"][0].clone();
        bad["dictionary"].as_array_mut().unwrap().push(extra);assert!(corrupt(&bad));
        let mut bad=wire.clone();bad["dictionary"][0]["hash"]=serde_json::json!("f".repeat(64));assert!(corrupt(&bad));
        let mut bad=wire.clone();bad["dictionary"].as_array_mut().unwrap().push(
            serde_json::json!({"file_id":2,"hash":"e".repeat(64)}));assert!(corrupt(&bad));
        let mut bad=wire.clone();bad["records"][0]["inventory"]["files"][0]["hash"]=serde_json::json!("f".repeat(64));
        assert!(corrupt(&bad));
        assert!(Catalogue::parse(&encoded,Format::Json).is_err());
    }
    #[test]
    fn csv_remains_rectangular_filterable_and_resilient_to_row_sorting() {
        let value=history();
        for mode in [Format::Csv,Format::CsvDedupeFile] {
            let bytes=value.encode(mode).unwrap();let mut reader=csv::Reader::from_reader(bytes.as_slice());
            assert_eq!(reader.headers().unwrap().len(),HEADER.len());
            let mut rows:Vec<_>=reader.records().map(|row|row.unwrap()).collect();
            let filtered:Vec<_>=rows.iter().filter(|row|&row[5]=="raw-data"&&&row[6]=="v1"&&&row[3]=="file")
                .map(|row|row[9].to_string()).collect();
            assert_eq!(filtered,directory_canonical().files.iter().map(|file|file.path.clone()).collect::<Vec<_>>());
            assert_eq!(rows.iter().filter(|r|&r[6]=="v2"&&&r[3]=="file").count(),0);
            assert_eq!(rows.iter().find(|r|&r[6]=="v2"&&&r[3]=="reference").unwrap().get(11),Some("v1"));
            rows.reverse();let mut writer=csv::Writer::from_writer(Vec::new());writer.write_record(HEADER).unwrap();
            for row in rows {writer.write_record(&row).unwrap();}
            assert_eq!(Catalogue::parse(&writer.into_inner().unwrap(),mode).unwrap(),value);
        }
    }
    #[test]
    fn malformed_membership_and_unknown_schema_fail_without_legacy_guessing() {
        let value=history();let bytes=value.encode(Format::Json).unwrap();
        let wire:serde_json::Value=serde_json::from_slice(&bytes).unwrap();
        let corrupt=|wire:&serde_json::Value|Catalogue::parse(&serde_json::to_vec(wire).unwrap(),Format::Json).is_err();
        let mut bad=wire.clone();bad["schema"]=serde_json::json!("legacy");assert!(corrupt(&bad));
        let mut bad=wire.clone();let duplicate=bad["records"][0]["inventory"]["files"][0].clone();
        bad["records"][0]["inventory"]["files"].as_array_mut().unwrap().push(duplicate);assert!(corrupt(&bad));
        let mut bad=wire.clone();bad["records"][0]["inventory"]["files"][0]["path"]=serde_json::json!("../escape");
        assert!(corrupt(&bad));
        let mut bad=wire.clone();bad["records"][1]["inventory"]["files"]=serde_json::json!([]);assert!(corrupt(&bad));
        assert!(Catalogue::parse(b"dir,file,hash\noutput,a,123\n",Format::Csv).is_err());
        let mut broken=history();broken.records[1].label="RAW-DATA".into();assert!(broken.validate().is_err());
        assert!(Catalogue::parse(&vec![b' ';MAX_METADATA_BYTES+1],Format::Json).is_err());
    }
}
