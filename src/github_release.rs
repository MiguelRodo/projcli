//! Native GitHub Release asset transport. ZIPs are opaque at this layer.
use std::{collections::BTreeSet, fs, io::{Read, Write, Seek}, net::IpAddr, time::Duration};
use anyhow::Context;
use camino::Utf8Path;
use serde::{Deserialize, Serialize};
use ureq::http::{Response, Uri};

/// Public metadata for one Release asset; no credentials or signed URLs.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReleaseAsset {
    /// GitHub's asset identifier.
    pub id: u64,
    /// Exact asset filename.
    pub name: String,
    /// Advertised binary size.
    pub size: u64,
}
#[derive(Deserialize)]
struct Release { id: u64, tag_name: String, upload_url: Option<String> }

/// GitHub REST client with streaming file bodies and no automatic redirects.
/// Tokens are private, omitted from this client's errors, and never sent on a download
/// redirect. HTTPS is required except for literal loopback fixture endpoints.
/// Listing/download can be anonymous; upload requires an existing token.
/// No credential installation or implicit overwrite occurs. Asset deletion is
/// restricted to an explicitly requested replacement.
/// Embedders must not enable ureq DEBUG/TRACE logging for sensitive requests:
/// dependency wire/URI logging is not redacted. Redirect targets are not a
/// network sandbox; caller-selected HTTPS services can redirect to other hosts.
pub struct GithubReleaseClient {
    agent: ureq::Agent,
    api: String,
    repository: String,
    token: Option<String>,
}

fn uri(text: &str) -> anyhow::Result<Uri> {
    let uri: Uri = text.parse().map_err(|_| anyhow::anyhow!("Invalid GitHub transport URL"))?;
    anyhow::ensure!(uri.authority().is_some() && !uri.authority().unwrap().as_str().contains('@'),
        "GitHub transport URL must have an authority without credentials");
    let loopback = uri.host().and_then(|host| host.trim_matches(['[', ']']).parse::<IpAddr>().ok())
        .is_some_and(|ip| ip.is_loopback());
    anyhow::ensure!(uri.scheme_str() == Some("https") || (uri.scheme_str() == Some("http") && loopback),
        "GitHub transport requires HTTPS (literal loopback HTTP is reserved for fixtures)");
    Ok(uri)
}
fn same_origin(left: &Uri, right: &Uri) -> bool {
    left.scheme_str() == right.scheme_str()
        && left.host().map(str::to_ascii_lowercase) == right.host().map(str::to_ascii_lowercase)
        && left.port_u16().unwrap_or(if left.scheme_str() == Some("https") {443} else {80})
            == right.port_u16().unwrap_or(if right.scheme_str() == Some("https") {443} else {80})
}
fn encode(text: &str) -> String {
    let mut encoded = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) { encoded.push(byte as char); }
        else { encoded.push_str(&format!("%{byte:02X}")); }
    }
    encoded
}
fn asset_name(name: &str) -> anyhow::Result<()> {
    crate::path_safety::validate_directory_label(name)
}
fn json<T: serde::de::DeserializeOwned>(response: Response<ureq::Body>) -> anyhow::Result<T> {
    let mut bytes = Vec::new();
    response.into_body().into_reader().take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)
        .map_err(|_| anyhow::anyhow!("Cannot read GitHub JSON response"))?;
    anyhow::ensure!(bytes.len() <= 4 * 1024 * 1024, "GitHub JSON response exceeds metadata limit");
    serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("Invalid GitHub JSON response"))
}

impl GithubReleaseClient {
    /// Creates a client for owner/repository and an explicit REST API base.
    /// A base path such as /api/v3 is preserved. URL queries/fragments, invalid
    /// repositories, token newlines and non-HTTPS non-loopback bases are errors.
    pub fn new(repository: &str, api: &str, token: Option<String>) -> anyhow::Result<Self> {
        let parts: Vec<_> = repository.split('/').collect();
        anyhow::ensure!(parts.len() == 2 && parts.iter().all(|part| !part.is_empty()
            && *part != "." && *part != ".." && part.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))),
            "GitHub repository must be owner/name");
        anyhow::ensure!(!api.contains(['?', '#']), "GitHub API base cannot contain a query or fragment");
        uri(api)?;
        let token = token.filter(|token| !token.trim().is_empty());
        anyhow::ensure!(token.as_ref().is_none_or(|token| !token.chars().any(char::is_control)),
            "Invalid GitHub token format");
        let config = ureq::Agent::config_builder().max_redirects(0).http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(120)))
            .user_agent(concat!("projcli/", env!("CARGO_PKG_VERSION"))).build();
        Ok(Self { agent: config.into(), api: api.trim_end_matches('/').into(),
            repository: repository.into(), token })
    }
    fn get(&self, path: &str, accept: &str) -> anyhow::Result<Response<ureq::Body>> {
        let mut request = self.agent.get(format!("{}{}", self.api, path))
            .header("Accept", accept).header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(token) = &self.token { request = request.header("Authorization", format!("Bearer {token}")); }
        request.call().map_err(|_| anyhow::anyhow!("GitHub API request failed"))
    }
    fn release(&self, tag: &str, create: bool) -> anyhow::Result<Release> {
        anyhow::ensure!(!tag.is_empty() && !tag.chars().any(char::is_control), "Invalid GitHub Release tag");
        let response = self.get(&format!("/repos/{}/releases/tags/{}", self.repository, encode(tag)), "application/vnd.github+json")?;
        let status = response.status().as_u16();
        let release: Release = if status == 200 { json(response)? } else {
            anyhow::ensure!(status == 404 && create, "GitHub Release lookup failed (HTTP {status})");
            let token = self.token.as_ref().context("Release creation requires an existing GitHub token")?;
            let response = self.agent.post(format!("{}/repos/{}/releases", self.api, self.repository))
                .header("Accept", "application/vnd.github+json").header("Authorization", format!("Bearer {token}"))
                .header("X-GitHub-Api-Version", "2022-11-28")
                .send_json(serde_json::json!({"tag_name":tag,"name":tag,"draft":false,"prerelease":false}))
                .map_err(|_| anyhow::anyhow!("GitHub Release creation request failed; publication outcome may be uncertain, inspect remote state before retrying"))?;
            let status = response.status().as_u16();
            anyhow::ensure!(status == 201, "GitHub Release creation failed (HTTP {status}); inspect remote state before retrying");
            json(response).context("Release creation may have completed; cannot validate response")?
        };
        anyhow::ensure!(release.id != 0 && release.tag_name == tag, "GitHub Release identity does not match request");
        Ok(release)
    }
    fn assets_for_release(&self, release: u64) -> anyhow::Result<Vec<ReleaseAsset>> {
        let mut assets = Vec::new();
        let mut ids = BTreeSet::new();
        for page in 1..=1000 {
            let response = self.get(&format!("/repos/{}/releases/{release}/assets?per_page=100&page={page}", self.repository), "application/vnd.github+json")?;
            let status = response.status().as_u16();
            anyhow::ensure!(status == 200, "GitHub asset listing failed (HTTP {status})");
            let batch: Vec<ReleaseAsset> = json(response)?;
            let count = batch.len();
            for asset in batch {
                anyhow::ensure!(asset.id != 0 && ids.insert(asset.id), "Duplicate/invalid GitHub asset identifier");
                assets.push(asset);
            }
            if count < 100 { return Ok(assets); }
        }
        anyhow::bail!("GitHub asset listing exceeds page limit")
    }
    /// Read-only catalog lookup for configured publication. A missing exact-tag
    /// Release returns None; other HTTP/identity failures remain errors.
    pub(crate) fn assets_if_exists(&self,tag:&str)->anyhow::Result<Option<Vec<ReleaseAsset>>> {
        anyhow::ensure!(!tag.is_empty()&&!tag.chars().any(char::is_control),"Invalid GitHub Release tag");
        let response=self.get(&format!("/repos/{}/releases/tags/{}",self.repository,encode(tag)),"application/vnd.github+json")?;
        let status=response.status().as_u16();
        if status==404 {return Ok(None);}
        anyhow::ensure!(status==200,"GitHub Release lookup failed (HTTP {status})");
        let release:Release=json(response)?;
        anyhow::ensure!(release.id!=0&&release.tag_name==tag,"GitHub Release identity does not match request");
        Ok(Some(self.assets_for_release(release.id)?))
    }
    /// Lists every page of assets on an existing exact-tag Release.
    pub fn assets(&self, tag: &str) -> anyhow::Result<Vec<ReleaseAsset>> {
        self.assets_for_release(self.release(tag, false)?.id)
    }
    /// Uploads one regular file as a new asset. Duplicate names fail without
    /// deleting anything. Creating a missing Release requires create_release.
    /// Publication is not rolled back on a later protocol/network failure.
    pub fn upload_asset(&self, tag: &str, name: &str, source: &Utf8Path, create_release: bool) -> anyhow::Result<ReleaseAsset> {
        self.upload_asset_impl(tag,name,source,create_release,false,None)
    }
    /// Explicitly replaces a matching asset using projr's delete-then-upload
    /// sequence, or uploads normally if absent. Stages the full local file and
    /// validates the upload endpoint before deletion. This is not atomic: a
    /// failed delete/upload can leave the old asset absent or the outcome
    /// uncertain. Errors state that boundary; no automatic retry/rollback occurs.
    pub fn replace_asset(&self, tag: &str, name: &str, source: &Utf8Path, create_release: bool) -> anyhow::Result<ReleaseAsset> {
        self.upload_asset_impl(tag,name,source,create_release,true,None)
    }
    /// Replace only the exact metadata asset observed while preparing history.
    /// A changed/new asset fails without deleting it; no automatic retry or lock claim.
    pub(crate) fn replace_observed_asset(&self,tag:&str,name:&str,source:&Utf8Path,expected:Option<u64>)->anyhow::Result<ReleaseAsset> {
        self.upload_asset_impl(tag,name,source,true,true,Some(expected))
    }
    fn upload_asset_impl(&self, tag: &str, name: &str, source: &Utf8Path, create_release: bool, replace: bool,
        expected:Option<Option<u64>>) -> anyhow::Result<ReleaseAsset> {
        asset_name(name)?;
        crate::path_safety::resolve_path(source.as_std_path(), true)?;
        anyhow::ensure!(fs::symlink_metadata(source)?.is_file(), "Asset source must be a regular file");
        let mut file = fs::File::open(source).with_context(|| format!("Cannot open asset source {source}"))?;
        let metadata = file.metadata()?;
        anyhow::ensure!(metadata.is_file(), "Asset source must be a regular file");
        anyhow::ensure!(self.token.is_some(), "Asset upload requires an existing GitHub token");
        // ponytail: private full-file staging costs disk space but keeps source
        // read failures before destructive replacement; no remote transaction.
        let mut staged=if replace {
            Some(tempfile::Builder::new().prefix("proj-replace-").tempfile_in(fs::canonicalize(std::env::temp_dir())?)?)
        } else {None};
        if let Some(staged)=&mut staged {
            let copied=std::io::copy(&mut file,staged.as_file_mut()).context("Cannot stage replacement; no asset was deleted")?;
            anyhow::ensure!(copied==metadata.len(),"Source size changed during replacement staging; no asset was deleted");
            staged.as_file_mut().flush()?;staged.as_file_mut().rewind()?;
        }
        let release = self.release(tag, create_release)?;
        let matches:Vec<_>=self.assets_for_release(release.id)?.into_iter().filter(|asset|asset.name==name).collect();
        anyhow::ensure!(matches.len()<=1,"Asset name is ambiguous; no deletion performed");
        if let Some(expected)=expected {
            anyhow::ensure!(matches.first().map(|asset|asset.id)==expected,
                "GitHub metadata asset changed during preparation; no deletion performed; inspect before retry");
        }
        anyhow::ensure!(replace||matches.is_empty(),"GitHub asset already exists; no overwrite or deletion performed");
        let template = release.upload_url.context("GitHub Release has no upload endpoint")?;
        let upload = template.split('{').next().context("Invalid GitHub upload endpoint")?;
        let upload_uri = uri(upload)?;
        let api_uri = uri(&self.api)?;
        let public_upload = api_uri.host().is_some_and(|host| host.eq_ignore_ascii_case("api.github.com")) && api_uri.scheme_str() == Some("https")
            && api_uri.port_u16().unwrap_or(443) == 443 && upload_uri.host().is_some_and(|host| host.eq_ignore_ascii_case("uploads.github.com"))
            && upload_uri.scheme_str() == Some("https") && upload_uri.port_u16().unwrap_or(443) == 443;
        anyhow::ensure!(same_origin(&api_uri, &upload_uri) || public_upload, "Untrusted GitHub upload endpoint");
        anyhow::ensure!(upload_uri.query().is_none(), "GitHub upload endpoint has an unexpected query");
        let deleted=if let Some(previous)=matches.first() {
            let response=self.agent.delete(format!("{}/repos/{}/releases/assets/{}",self.api,self.repository,previous.id))
                .header("Accept","application/vnd.github+json")
                .header("Authorization",format!("Bearer {}",self.token.as_ref().unwrap()))
                .header("X-GitHub-Api-Version","2022-11-28").call()
                .map_err(|_|anyhow::anyhow!("Asset deletion request failed; old asset may have been deleted, replacement was not uploaded; inspect remote state before retrying"))?;
            let status=response.status().as_u16();
            anyhow::ensure!(status==204,"Asset deletion failed (HTTP {status}); old asset state is unconfirmed, replacement was not uploaded; inspect remote state before retrying");
            true
        } else {false};
        let result=(|| {
        let input:&mut dyn Read=match &mut staged {Some(staged)=>staged.as_file_mut(),None=>&mut file};
        let response = self.agent.post(format!("{upload}?name={}", encode(name)))
            .header("Accept", "application/vnd.github+json")
            .header("Authorization", format!("Bearer {}", self.token.as_ref().unwrap()))
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("Content-Type", "application/octet-stream")
            .header("Content-Length", metadata.len().to_string())
            .send(ureq::SendBody::from_reader(input))
            .map_err(|_| anyhow::anyhow!("Asset upload request failed; remote outcome may be uncertain"))?;
        let status = response.status().as_u16();
        anyhow::ensure!(status == 201, "GitHub asset upload failed (HTTP {status}); inspect remote state before retrying");
        let asset: ReleaseAsset = json(response).context("Upload may have completed; cannot validate response")?;
        anyhow::ensure!(asset.id != 0 && asset.name == name && asset.size == metadata.len(),
            "Uploaded asset metadata mismatch; inspect remote state before retrying");
        anyhow::ensure!(!deleted || matches.first().is_none_or(|old|old.id!=asset.id),
            "Replacement response reused the deleted asset identity; inspect remote state before retrying");
        Ok(asset)
        })();
        if deleted { result.context("Previous asset was deleted; inspect remote state if the replacement was not validated") }
        else {result}
    }
    /// Removes one exact named asset after fresh unambiguous lookup. Intended for
    /// obsolete empty variants after their full replacement has been validated.
    /// No retry or rollback: failure reports an uncertain remote deletion.
    pub(crate) fn delete_asset(&self,tag:&str,name:&str)->anyhow::Result<()> {
        asset_name(name)?;
        let token=self.token.as_ref().context("Asset deletion requires an existing GitHub token")?;
        let release=self.release(tag,false)?;
        let matches:Vec<_>=self.assets_for_release(release.id)?.into_iter().filter(|asset|asset.name==name).collect();
        anyhow::ensure!(matches.len()==1,"Asset deletion requires exactly one matching name; no deletion performed");
        let response=self.agent.delete(format!("{}/repos/{}/releases/assets/{}",self.api,self.repository,matches[0].id))
            .header("Accept","application/vnd.github+json").header("Authorization",format!("Bearer {token}"))
            .header("X-GitHub-Api-Version","2022-11-28").call()
            .map_err(|_|anyhow::anyhow!("Asset deletion request failed; remote state is uncertain; inspect before retrying"))?;
        anyhow::ensure!(response.status().as_u16()==204,"Asset deletion failed (HTTP {}); remote state is uncertain; inspect before retrying",response.status().as_u16());
        Ok(())
    }
    /// Streams an exact named asset into a same-directory temporary file, then
    /// replaces an absent/regular destination with a new inode. Guards reject
    /// links, special files and project metadata. Truncation/size-limit/network
    /// failures preserve old bytes. Redirects carry no Authorization header.
    /// Existing permissions survive; new files are private tempfile defaults.
    /// No archive extraction, MD5 verification or concurrent-writer guarantee.
    pub fn download_asset(&self, root: &Utf8Path, tag: &str, name: &str, destination: &Utf8Path, max_bytes: u64) -> anyhow::Result<ReleaseAsset> {
        asset_name(name)?;
        validate_destination(root, destination)?;
        let permissions = match fs::symlink_metadata(destination) {
            Ok(metadata) => {
                #[cfg(windows)]
                anyhow::ensure!(!metadata.permissions().readonly(), "Cannot safely replace an existing read-only asset destination");
                Some(metadata.permissions())
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let matches: Vec<_> = self.assets(tag)?.into_iter().filter(|asset| asset.name == name).collect();
        anyhow::ensure!(matches.len() == 1, "Requested GitHub asset is missing or ambiguous");
        let asset = matches.into_iter().next().unwrap();
        anyhow::ensure!(asset.size <= max_bytes, "GitHub asset exceeds the requested byte limit");
        let mut response = self.get(&format!("/repos/{}/releases/assets/{}", self.repository, asset.id), "application/octet-stream")?;
        for _ in 0..5 {
            if response.status().as_u16() == 200 { break; }
            anyhow::ensure!(response.status().as_u16() == 302, "GitHub asset download failed (HTTP {})", response.status().as_u16());
            let location = response.headers().get("Location").and_then(|value| value.to_str().ok()).context("Asset redirect has no valid location")?;
            let redirect = uri(location)?;
            let api = uri(&self.api)?;
            anyhow::ensure!(redirect.scheme_str() == Some("https") || api.scheme_str() == Some("http"),
                "Asset redirect cannot downgrade HTTPS");
            response = self.agent.get(location).header("Accept", "application/octet-stream").call()
                .map_err(|_| anyhow::anyhow!("Redirected asset download request failed"))?;
        }
        anyhow::ensure!(response.status().as_u16() == 200, "GitHub asset redirect limit exceeded");
        let parent = destination.parent().context("Asset destination has no parent")?;
        let mut staged = tempfile::Builder::new().prefix(".proj-asset-").tempfile_in(parent)?;
        let actual = std::io::copy(&mut response.into_body().into_reader().take(asset.size.saturating_add(1)), &mut staged)
            .map_err(|_| anyhow::anyhow!("Cannot complete asset download; destination preserved"))?;
        anyhow::ensure!(actual == asset.size, "Downloaded asset size differs from advertised size; destination preserved");
        staged.flush()?;
        if let Some(permissions) = permissions { fs::set_permissions(staged.path(), permissions)?; }
        validate_destination(root, destination)?;
        staged.persist(destination).map_err(|error| error.error).context("Cannot install complete asset download")?;
        Ok(asset)
    }
}
fn validate_destination(root: &Utf8Path, destination: &Utf8Path) -> anyhow::Result<()> {
    // Reject Win32 aliases before resolving prefixes into verbatim paths, which
    // otherwise changes the meaning of trailing spaces/dots on Windows.
    anyhow::ensure!(!destination.components().any(|part| {
        matches!(part, camino::Utf8Component::Normal(name) if name.ends_with(['.', ' ']))
    }), "Asset destination contains an unsafe native path alias");
    let identity = crate::path_safety::resolve_path(destination.as_std_path(), true)?;
    let root_identity = crate::path_safety::resolve_path(root.as_std_path(), false)?;
    if let Ok(relative) = identity.strip_prefix(&root_identity) {
        anyhow::ensure!(!relative.components().any(|part| [".git", ".svn", ".projr"]
            .iter().any(|name| part.as_os_str().to_string_lossy().eq_ignore_ascii_case(name))),
            "Asset destination overlaps protected project state");
    }
    for name in [".git", ".svn", ".projr"] {
        let protected = root_identity.join(name);
        anyhow::ensure!(!identity.starts_with(&protected) && !protected.starts_with(&identity),
            "Asset destination overlaps protected project state");
    }
    let parent = destination.parent().context("Asset destination has no parent")?;
    let at_root = crate::path_safety::resolve_path(parent.as_std_path(), false)? == root_identity;
    let protected = at_root && [destination.file_name(), identity.file_name().and_then(|name| name.to_str())].into_iter().flatten().any(|name| {
        let name = name.to_ascii_lowercase();
        matches!(name.as_str(), "version"|"manifest.csv"|"_proj.yml"|"_projr.yml")
            || (name.starts_with("_projr-") && name.ends_with(".yml"))
    });
    anyhow::ensure!(!protected, "Asset destination is a protected project authority file");
    let parent = destination.parent().context("Asset destination has no parent")?;
    anyhow::ensure!(fs::symlink_metadata(parent)?.is_dir(), "Asset destination parent must exist");
    match fs::symlink_metadata(destination) {
        Ok(metadata) => anyhow::ensure!(metadata.is_file(), "Asset destination must be absent or a regular file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
        Err(error) => return Err(error.into()),
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn path_encoding_and_transport_origins_are_bounded() {
        assert_eq!(encode("archive/v1.2.3"),"archive%2Fv1.2.3");
        assert!(uri("https://user:secret@example.com/api").is_err());
        assert!(uri("http://example.com").is_err());
        assert!(uri("http://127.0.0.1:1234").is_ok());
        assert!(!same_origin(&uri("https://api.github.com").unwrap(), &uri("https://other.example").unwrap()));
    }
}
