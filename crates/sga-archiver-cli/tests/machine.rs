//! The machine-readable commands are a contract other programs parse: exactly
//! one JSON object on stdout, a nonzero exit and a stderr message on failure,
//! and no member bytes read that the command did not need.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use sga::entries::{FileEncryptionType, FileStorageType, FileVerificationType, HeaderReserved};
use sga::{Archive, FileEntry, Folder, Toc, TocLayout};

const RGD_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../relic-chunky/tests/weapon_war_elephant_spear_3_sul.rgd"
);

struct Sandbox(PathBuf);

impl Sandbox {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "sga-archiver-cli-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        Sandbox(dir)
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn empty(name: &str) -> Archive {
    Archive {
        header_reserved: HeaderReserved::default(),
        name: name.into(),
        version: 10,
        product: 0,
        block_size: 8,
        header_encryption_type: FileEncryptionType::None,
        signature: [0; 256],
        layout: TocLayout::Modern,
        tocs: Vec::new(),
    }
}

fn write_archive(archive: &Archive, path: &Path) {
    let mut file = fs::File::create(path).unwrap();
    archive.write(&mut file).unwrap();
}

/// A base archive with art, script and attribute members, one compressed.
fn base_archive(path: &Path) {
    let mut archive = empty("0123456789abcdef0123456789abcdef");
    archive.upsert_stored_in(
        "data",
        "art/house.rgm",
        b"geometry that is long enough to compress".to_vec(),
    );
    archive.upsert_stored_in("data", "art/house.rrmaterial", b"material".to_vec());
    archive.upsert_stored_in("data", "scar/main.scar", b"-- lua".to_vec());
    archive.upsert_stored_in("attrib", "attrib/unit.rgd", fs::read(RGD_FIXTURE).unwrap());
    archive.repackage_art().unwrap();
    write_archive(&archive, path);
}

fn run(args: &[&str]) -> (bool, Value, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_sga-archiver"))
        .args(args)
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    let value = if output.status.success() {
        serde_json::from_str(&stdout).unwrap_or_else(|error| {
            panic!("stdout of {args:?} is not one JSON object: {error}\n{stdout}")
        })
    } else {
        assert!(
            stdout.is_empty(),
            "failed {args:?} still wrote stdout:\n{stdout}"
        );
        Value::Null
    };
    (output.status.success(), value, stderr)
}

fn ok(args: &[&str]) -> Value {
    let (success, value, stderr) = run(args);
    assert!(success, "{args:?} failed:\n{stderr}");
    assert_eq!(value["tool"]["name"], "sga-archiver");
    assert_eq!(value["tool"]["version"], env!("CARGO_PKG_VERSION"));
    value
}

fn fails(args: &[&str]) -> String {
    let (success, _, stderr) = run(args);
    assert!(!success, "{args:?} unexpectedly succeeded");
    stderr
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn member_paths(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            format!(
                "{}:{}",
                m["toc"].as_str().unwrap(),
                m["path"].as_str().unwrap()
            )
        })
        .collect()
}

#[test]
fn inspect_names_members_with_forward_slashes() {
    let sandbox = Sandbox::new("inspect");
    let archive = sandbox.path("base.sga");
    base_archive(&archive);

    let value = ok(&["inspect", &s(&archive)]);
    assert_eq!(value["schema"], "sga-archiver.inspect/v1");
    let summary = &value["archive"];
    assert_eq!(summary["name"], "0123456789abcdef0123456789abcdef");
    assert_eq!(summary["version"], 10);
    assert_eq!(summary["file_count"], 4);
    assert_eq!(summary["toc_count"], 2);
    assert_eq!(summary["signature_present"], false);
    assert_eq!(summary["header_encryption"], "None");
    let members = member_paths(&summary["members"]);
    assert!(
        members.contains(&"data:art/house.rgm".to_string()),
        "{members:?}"
    );
    assert!(
        members.contains(&"attrib:attrib/unit.rgd".to_string()),
        "{members:?}"
    );
}

#[test]
fn list_reports_storage_details_and_filters_case_insensitively() {
    let sandbox = Sandbox::new("list");
    let archive = sandbox.path("base.sga");
    base_archive(&archive);

    let value = ok(&["list", &s(&archive)]);
    assert_eq!(value["schema"], "sga-archiver.list/v1");
    assert_eq!(value["member_count"], 4);
    assert!(value["archive"]["verification"].is_null());
    assert!(
        value["archive"].get("members").is_none(),
        "list carries members at top level only"
    );
    let rgm = value["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["path"] == "art/house.rgm")
        .unwrap();
    assert_eq!(rgm["storage"], "BufferCompress");
    assert_eq!(rgm["encryption"], "None");
    assert_eq!(rgm["verification"], "None");
    assert_eq!(rgm["size"], 40);
    assert!(rgm["stored_size"].as_u64().unwrap() > 0);
    let scar = value["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["path"] == "scar/main.scar")
        .unwrap();
    assert_eq!(scar["storage"], "Store");
    assert_eq!(scar["verification"], "SHA1Blocks");
    assert_eq!(scar["toc"], "data");

    let filtered = ok(&["list", &s(&archive), "--filter", ".RRMATERIAL"]);
    assert_eq!(filtered["member_count"], 1);
    assert_eq!(filtered["filter"], ".RRMATERIAL");
    assert_eq!(filtered["members"][0]["path"], "art/house.rrmaterial");
}

#[test]
fn list_verify_checks_stored_bytes() {
    let sandbox = Sandbox::new("verify");
    let archive = sandbox.path("base.sga");
    base_archive(&archive);

    let value = ok(&["list", &s(&archive), "--verify"]);
    let verification = &value["archive"]["verification"];
    assert_eq!(verification["verified"], true);
    assert_eq!(verification["checked_files"], 4);
    assert_eq!(verification["crc_mismatches"].as_array().unwrap().len(), 0);

    // Corrupt the stored bytes of the .scar member and check again.
    let mut bytes = fs::read(&archive).unwrap();
    let scar_offset = bytes.windows(6).position(|w| w == b"-- lua").unwrap();
    bytes[scar_offset] ^= 0xFF;
    let corrupted = sandbox.path("corrupted.sga");
    fs::write(&corrupted, bytes).unwrap();
    let value = ok(&["list", &s(&corrupted), "--verify"]);
    let verification = &value["archive"]["verification"];
    assert_eq!(verification["verified"], false);
    assert_eq!(verification["crc_mismatches"][0]["path"], "scar/main.scar");
    assert_eq!(verification["sha1_mismatches"][0]["path"], "scar/main.scar");
}

#[test]
fn extract_member_writes_decoded_bytes_and_refuses_overwrite() {
    let sandbox = Sandbox::new("extract-member");
    let archive = sandbox.path("base.sga");
    base_archive(&archive);
    let output = sandbox.path("out/house.rgm");

    let value = ok(&[
        "extract-member",
        &s(&archive),
        "ART\\House.rgm",
        &s(&output),
    ]);
    assert_eq!(value["schema"], "sga-archiver.extract-member/v1");
    assert_eq!(value["toc"], "data");
    assert_eq!(value["path"], "art/house.rgm");
    assert_eq!(value["size"], 40);
    assert_eq!(
        fs::read(&output).unwrap(),
        b"geometry that is long enough to compress"
    );

    let stderr = fails(&["extract-member", &s(&archive), "art/house.rgm", &s(&output)]);
    assert!(stderr.contains("refusing to overwrite"), "{stderr}");
    let stderr = fails(&[
        "extract-member",
        &s(&archive),
        "art/missing.rgm",
        &s(&sandbox.path("x")),
    ]);
    assert!(stderr.contains("not found"), "{stderr}");
}

#[test]
fn extract_member_refuses_encrypted_members() {
    let sandbox = Sandbox::new("encrypted");
    let mut archive = empty("0123456789abcdef0123456789abcdef");
    archive.tocs.push(Toc {
        alias: "data".into(),
        name: "data".into(),
        root: Folder {
            name: String::new(),
            files: vec![FileEntry {
                name: "secret.bin".into(),
                stored_data: b"ciphertext".to_vec(),
                uncompressed_size: 10,
                storage_type: FileStorageType::Store,
                encryption_type: FileEncryptionType::Aes128,
                verification_type: FileVerificationType::None,
                crc: 0,
                data_order: None,
            }],
            folders: vec![],
        },
    });
    let path = sandbox.path("secret.sga");
    write_archive(&archive, &path);

    let listed = ok(&["list", &s(&path)]);
    assert_eq!(listed["members"][0]["encryption"], "Aes128");

    let stderr = fails(&[
        "extract-member",
        &s(&path),
        "secret.bin",
        &s(&sandbox.path("out.bin")),
    ]);
    assert!(stderr.contains("Aes128"), "{stderr}");
    assert!(!sandbox.path("out.bin").exists());

    let bulk = ok(&[
        "extract",
        &s(&path),
        &s(&sandbox.path("bulk")),
        "--filter",
        ".bin",
    ]);
    assert_eq!(bulk["written_count"], 0);
    assert_eq!(bulk["encrypted"][0]["path"], "secret.bin");
}

#[test]
fn extract_selects_by_filter_or_member_and_keeps_paths() {
    let sandbox = Sandbox::new("extract");
    let archive = sandbox.path("base.sga");
    base_archive(&archive);
    let out = sandbox.path("slot");

    let value = ok(&["extract", &s(&archive), &s(&out), "--filter", "art/"]);
    assert_eq!(value["schema"], "sga-archiver.extract/v1");
    assert_eq!(value["written_count"], 2);
    assert!(out.join("art").join("house.rgm").is_file());
    assert!(out.join("art").join("house.rrmaterial").is_file());
    assert!(!out.join("scar").exists());

    let value = ok(&[
        "extract",
        &s(&archive),
        &s(&sandbox.path("picked")),
        "--member",
        "scar\\main.scar",
        "--member",
        "ATTRIB/unit.rgd",
    ]);
    assert_eq!(value["written_count"], 2);
    assert_eq!(
        fs::read(sandbox.path("picked/scar/main.scar")).unwrap(),
        b"-- lua"
    );
    assert!(sandbox.path("picked/attrib/unit.rgd").is_file());

    let stderr = fails(&[
        "extract",
        &s(&archive),
        &s(&sandbox.path("none")),
        "--member",
        "nope.bin",
    ]);
    assert!(stderr.contains("not found"), "{stderr}");
    let stderr = fails(&["extract", &s(&archive), &s(&sandbox.path("none"))]);
    assert!(stderr.contains("--filter"), "{stderr}");
    let stderr = fails(&["extract", &s(&archive), &s(&out), "--filter", "art/"]);
    assert!(stderr.contains("refusing to overwrite"), "{stderr}");
}

#[test]
fn decode_and_patch_rgd() {
    let sandbox = Sandbox::new("rgd");
    let xml = sandbox.path("unit.xml");
    let value = ok(&["decode-rgd", RGD_FIXTURE, &s(&xml)]);
    assert_eq!(value["schema"], "sga-archiver.decode-rgd/v1");
    let text = fs::read_to_string(&xml).unwrap();
    assert!(text.contains("Spearman Weapon"), "{text}");

    let patched = sandbox.path("unit.patched.rgd");
    let value = ok(&[
        "patch-rgd-cstring",
        RGD_FIXTURE,
        &s(&patched),
        "cdn_2h_spear",
        "owner:cdn_2h_spear",
        "2",
    ]);
    assert_eq!(value["replacements"], 2);
    // Two longer strings; alignment padding after them can shrink, so only
    // the direction is fixed.
    assert!(
        value["patched_aegd_size"].as_u64().unwrap()
            > value["original_aegd_size"].as_u64().unwrap(),
        "{value}"
    );
    let reparsed = sandbox.path("unit.patched.xml");
    ok(&["decode-rgd", &s(&patched), &s(&reparsed)]);
    let text = fs::read_to_string(&reparsed).unwrap();
    assert!(text.contains("owner:cdn_2h_spear"), "{text}");
    assert!(text.contains("Spearman Weapon"), "other values untouched");

    let stderr = fails(&[
        "patch-rgd-cstring",
        RGD_FIXTURE,
        &s(&sandbox.path("never.rgd")),
        "cdn_2h_spear",
        "x",
        "3",
    ]);
    assert!(
        stderr.contains("expected 3 CString replacements, found 2"),
        "{stderr}"
    );
    assert!(!sandbox.path("never.rgd").exists());

    let stderr = fails(&[
        "patch-rgd-f32",
        RGD_FIXTURE,
        &s(&sandbox.path("never.rgd")),
        "no_such_key",
        "1",
        "2",
        "1",
    ]);
    assert!(
        stderr.contains("expected 1 Float replacements, found 0"),
        "{stderr}"
    );
}

#[test]
fn repack_graft_replace_round_trip() {
    let sandbox = Sandbox::new("write");
    let base = sandbox.path("base.sga");
    base_archive(&base);

    let repacked = sandbox.path("repacked.sga");
    let value = ok(&["repack", &s(&base), &s(&repacked)]);
    assert_eq!(value["schema"], "sga-archiver.repack/v1");
    assert_eq!(fs::read(&base).unwrap(), fs::read(&repacked).unwrap());

    let mut donor = empty("fedcba9876543210fedcba9876543210");
    donor.upsert_stored_in("data", "art/custom.rgm", b"custom geometry".to_vec());
    donor.upsert_stored_in("data", "art/house.rrmaterial", b"donor material".to_vec());
    let donor_path = sandbox.path("donor.sga");
    write_archive(&donor, &donor_path);

    let grafted = sandbox.path("grafted.sga");
    let value = ok(&[
        "graft",
        &s(&base),
        &s(&donor_path),
        &s(&grafted),
        "art\\custom.rgm",
        "ART/house.rrmaterial",
    ]);
    assert_eq!(value["schema"], "sga-archiver.graft/v1");
    assert_eq!(value["grafted_members"][0], "art\\custom.rgm");
    let members = member_paths(&value["archive"]["members"]);
    assert!(
        members.contains(&"data:art/custom.rgm".to_string()),
        "{members:?}"
    );
    let listed = ok(&[
        "extract-member",
        &s(&grafted),
        "art/house.rrmaterial",
        &s(&sandbox.path("m.bin")),
    ]);
    assert_eq!(listed["size"], "donor material".len());
    assert_eq!(fs::read(sandbox.path("m.bin")).unwrap(), b"donor material");

    let renamed = sandbox.path("renamed.sga");
    let value = ok(&[
        "graft-as",
        &s(&base),
        &s(&donor_path),
        &s(&renamed),
        "art/custom.rgm=art/renamed.rgm",
    ]);
    assert_eq!(value["grafted_mappings"][0]["target"], "art/renamed.rgm");
    ok(&[
        "extract-member",
        &s(&renamed),
        "art/renamed.rgm",
        &s(&sandbox.path("r.bin")),
    ]);
    assert_eq!(fs::read(sandbox.path("r.bin")).unwrap(), b"custom geometry");
    let stderr = fails(&[
        "graft-as",
        &s(&base),
        &s(&donor_path),
        &s(&sandbox.path("bad.sga")),
        "no-equals",
    ]);
    assert!(stderr.contains("invalid graft-as mapping"), "{stderr}");

    let payload = sandbox.path("payload.bin");
    fs::write(&payload, b"replacement script").unwrap();
    let replaced = sandbox.path("replaced.sga");
    let value = ok(&[
        "replace-member",
        &s(&base),
        &s(&payload),
        &s(&replaced),
        "scar\\main.scar",
    ]);
    assert_eq!(value["replaced_member"], "scar\\main.scar");
    assert_eq!(value["payload"]["size"], "replacement script".len());
    ok(&[
        "extract-member",
        &s(&replaced),
        "scar/main.scar",
        &s(&sandbox.path("s.bin")),
    ]);
    assert_eq!(
        fs::read(sandbox.path("s.bin")).unwrap(),
        b"replacement script"
    );
    let stderr = fails(&[
        "replace-member",
        &s(&base),
        &s(&payload),
        &s(&sandbox.path("bad.sga")),
        "scar/absent.scar",
    ]);
    assert!(stderr.contains("not found"), "{stderr}");
}

#[test]
fn failures_leave_stdout_empty() {
    let sandbox = Sandbox::new("fail");
    let stderr = fails(&["inspect", &s(&sandbox.path("missing.sga"))]);
    assert!(stderr.contains("failed to read SGA"), "{stderr}");
}
