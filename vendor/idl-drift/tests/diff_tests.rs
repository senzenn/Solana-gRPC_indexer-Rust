//! Integration tests — assert the Rust engine's verdict matches the awk oracle
//! (proto/*.facts, 13/13 green) on equivalent JSON fixtures.
//!
//! Run: `cargo test`
//!
//! Each test builds two IDLs inline and checks (a) whether a breaking change is
//! present (the CI contract) and (b) the highest severity reported.

use idl_drift::codama_convert::decode_encoded_bytes;
use idl_drift::model::{
    Account, AccountItem, Event, Field, Instruction, Metadata, NamedDisc, Type, TypeDef, TypeDefTy,
};
use idl_drift::{diff, model::Idl, Severity};

fn parse(s: &str) -> Idl {
    Idl::from_json(s).expect("fixture should deserialize")
}

fn max_sev(old: &str, new: &str) -> Option<Severity> {
    diff(&parse(old), &parse(new))
        .changes
        .iter()
        .map(|c| c.severity)
        .max()
}

const AMM_OLD: &str = r#"{
  "address":"A","metadata":{"name":"amm","version":"0.1.0","spec":"0.1.0"},
  "instructions":[{"name":"swap","discriminator":[1,2,3,4,5,6,7,8],
    "accounts":[{"name":"pool","writable":true},{"name":"user","signer":true}],
    "args":[{"name":"amountIn","type":"u64"}]}],
  "accounts":[{"name":"Pool","discriminator":[9,9,9,9,9,9,9,9]}],
  "types":[{"name":"Pool","type":{"kind":"struct","fields":[
    {"name":"authority","type":"pubkey"},{"name":"feeBps","type":"u16"}]}}]
}"#;

const AMM_NEW: &str = r#"{
  "address":"A","metadata":{"name":"amm","version":"0.2.0","spec":"0.1.0"},
  "instructions":[{"name":"swap","discriminator":[1,2,3,4,5,6,7,8],
    "accounts":[{"name":"pool","writable":true},{"name":"user","signer":true},{"name":"feeVault","writable":true}],
    "args":[{"name":"amountIn","type":"u64"}]},
    {"name":"collectFees","discriminator":[8,7,6,5,4,3,2,1],"accounts":[{"name":"pool","writable":true}],"args":[]}],
  "accounts":[{"name":"Pool","discriminator":[9,9,9,9,9,9,9,9]}],
  "types":[{"name":"Pool","type":{"kind":"struct","fields":[
    {"name":"authority","type":"pubkey"},{"name":"feeBps","type":"u32"}]}}]
}"#;

#[test]
fn amm_pool_field_width_is_breaking() {
    // Pool.feeBps u16->u32 on a reachable account type => breaking (oracle: amm exit=1)
    let r = diff(&parse(AMM_OLD), &parse(AMM_NEW));
    assert!(r.has_breaking(), "expected breaking; got {:?}", r.changes);
    assert_eq!(r.exit_code(), 1);
}

const DEAD_OLD: &str = r#"{
  "address":"A","metadata":{"name":"d","version":"1","spec":"0.1.0"},
  "instructions":[{"name":"noop","discriminator":[1],"accounts":[],"args":[]}],
  "types":[{"name":"DeadType","type":{"kind":"struct","fields":[{"name":"x","type":"u16"}]}}]
}"#;
const DEAD_NEW: &str = r#"{
  "address":"A","metadata":{"name":"d","version":"1","spec":"0.1.0"},
  "instructions":[{"name":"noop","discriminator":[1],"accounts":[],"args":[]}],
  "types":[{"name":"DeadType","type":{"kind":"struct","fields":[{"name":"x","type":"u64"}]}}]
}"#;

#[test]
fn unreachable_type_change_is_not_breaking() {
    // oracle: dead exit=0
    let r = diff(&parse(DEAD_OLD), &parse(DEAD_NEW));
    assert!(
        !r.has_breaking(),
        "dead type must not be breaking; got {:?}",
        r.changes
    );
    assert_eq!(r.exit_code(), 0);
}

const ENUM_OLD: &str = r#"{
  "address":"A","metadata":{"name":"e","version":"1","spec":"0.1.0"},
  "instructions":[{"name":"upd","discriminator":[1],"accounts":[],"args":[{"name":"m","type":{"defined":{"name":"Mode"}}}]}],
  "types":[{"name":"Mode","type":{"kind":"enum","variants":[{"name":"Fast"},{"name":"Slow"}]}}]
}"#;
const ENUM_APPEND: &str = r#"{
  "address":"A","metadata":{"name":"e","version":"1","spec":"0.1.0"},
  "instructions":[{"name":"upd","discriminator":[1],"accounts":[],"args":[{"name":"m","type":{"defined":{"name":"Mode"}}}]}],
  "types":[{"name":"Mode","type":{"kind":"enum","variants":[{"name":"Fast"},{"name":"Slow"},{"name":"Paused"}]}}]
}"#;
const ENUM_INSERT: &str = r#"{
  "address":"A","metadata":{"name":"e","version":"1","spec":"0.1.0"},
  "instructions":[{"name":"upd","discriminator":[1],"accounts":[],"args":[{"name":"m","type":{"defined":{"name":"Mode"}}}]}],
  "types":[{"name":"Mode","type":{"kind":"enum","variants":[{"name":"Fast"},{"name":"Paused"},{"name":"Slow"}]}}]
}"#;

#[test]
fn enum_append_is_dangerous_not_breaking() {
    // oracle: enumappend exit=0
    let r = diff(&parse(ENUM_OLD), &parse(ENUM_APPEND));
    assert!(
        !r.has_breaking(),
        "append must not be breaking; got {:?}",
        r.changes
    );
    assert_eq!(max_sev(ENUM_OLD, ENUM_APPEND), Some(Severity::Dangerous));
}

#[test]
fn enum_middle_insert_is_breaking() {
    // oracle: enum exit=1 (Slow shifts tag 1->2)
    let r = diff(&parse(ENUM_OLD), &parse(ENUM_INSERT));
    assert!(
        r.has_breaking(),
        "insert must be breaking; got {:?}",
        r.changes
    );
}

const SER_OLD: &str = r#"{
  "address":"A","metadata":{"name":"s","version":"1","spec":"0.1.0"},
  "instructions":[{"name":"noop","discriminator":[1],"accounts":[],"args":[]}],
  "accounts":[{"name":"Vault","discriminator":[1,2,3,4,5,6,7,8]}],
  "types":[{"name":"Vault","serialization":"borsh","type":{"kind":"struct","fields":[{"name":"bal","type":"u64"}]}}]
}"#;
const SER_NEW: &str = r#"{
  "address":"A","metadata":{"name":"s","version":"1","spec":"0.1.0"},
  "instructions":[{"name":"noop","discriminator":[1],"accounts":[],"args":[]}],
  "accounts":[{"name":"Vault","discriminator":[1,2,3,4,5,6,7,8]}],
  "types":[{"name":"Vault","serialization":"bytemuck","type":{"kind":"struct","fields":[{"name":"bal","type":"u64"}]}}]
}"#;

#[test]
fn serialization_mode_change_is_breaking() {
    // oracle: serial exit=1
    let r = diff(&parse(SER_OLD), &parse(SER_NEW));
    assert!(
        r.has_breaking(),
        "borsh->bytemuck must be breaking; got {:?}",
        r.changes
    );
}

// Blocker #14: generic type-arg change. Wrapper<u64> -> Wrapper<u32> must be
// breaking. The old model dropped `generics`, so both sides deserialized equal
// and this was silently missed.
const GEN_OLD: &str = r#"{
  "address":"A","metadata":{"name":"g","version":"1","spec":"0.1.0"},
  "instructions":[{"name":"f","discriminator":[1],"accounts":[],
    "args":[{"name":"w","type":{"defined":{"name":"Wrapper","generics":[{"kind":"type","type":"u64"}]}}}]}],
  "types":[{"name":"Wrapper","type":{"kind":"struct","fields":[{"name":"inner","type":{"generic":"T"}}]}}]
}"#;
const GEN_NEW: &str = r#"{
  "address":"A","metadata":{"name":"g","version":"1","spec":"0.1.0"},
  "instructions":[{"name":"f","discriminator":[1],"accounts":[],
    "args":[{"name":"w","type":{"defined":{"name":"Wrapper","generics":[{"kind":"type","type":"u32"}]}}}]}],
  "types":[{"name":"Wrapper","type":{"kind":"struct","fields":[{"name":"inner","type":{"generic":"T"}}]}}]
}"#;

#[test]
fn generic_arg_change_is_breaking() {
    let r = diff(&parse(GEN_OLD), &parse(GEN_NEW));
    assert!(
        r.has_breaking(),
        "Wrapper<u64>->Wrapper<u32> must be breaking; got {:?}",
        r.changes
    );
}

// Blocker #12: native/SPL-style IDL with no discriminator must deserialize and
// not false-positive when unchanged.
const NODISC: &str = r#"{
  "address":"A","metadata":{"name":"n","version":"1","spec":"0.1.0"},
  "instructions":[{"name":"transfer","accounts":[],"args":[{"name":"amount","type":"u64"}]}]
}"#;

#[test]
fn missing_discriminator_deserializes_and_is_stable() {
    let idl = parse(NODISC); // must not panic (Blocker #12)
    let r = diff(&idl, &idl);
    assert!(!r.has_breaking());
    assert_eq!(r.exit_code(), 0);
}

// Blocker #15: type-alias target change. `Amount = u64` -> `Amount = u32` on a
// reachable alias is a Borsh-width break. Previously fell through diff_types'
// `_ => {}` arm and was silently missed.
const ALIAS_OLD: &str = r#"{
  "address":"A","metadata":{"name":"a","version":"1","spec":"0.1.0"},
  "instructions":[{"name":"x","discriminator":[1],"accounts":[],
    "args":[{"name":"amt","type":{"defined":{"name":"Amount"}}}]}],
  "types":[{"name":"Amount","type":{"kind":"type","alias":"u64"}}]
}"#;
const ALIAS_NEW: &str = r#"{
  "address":"A","metadata":{"name":"a","version":"1","spec":"0.1.0"},
  "instructions":[{"name":"x","discriminator":[1],"accounts":[],
    "args":[{"name":"amt","type":{"defined":{"name":"Amount"}}}]}],
  "types":[{"name":"Amount","type":{"kind":"type","alias":"u32"}}]
}"#;

#[test]
fn alias_target_change_is_breaking() {
    let r = diff(&parse(ALIAS_OLD), &parse(ALIAS_NEW));
    assert!(
        r.has_breaking(),
        "Amount=u64->u32 must be breaking; got {:?}",
        r.changes
    );
}

// Blocker #17: duplicate instruction names make name-keyed matching lossy.
// The tool must flag the duplication rather than silently drop entries.
const DUP: &str = r#"{
  "address":"A","metadata":{"name":"d","version":"1","spec":"0.1.0"},
  "instructions":[
    {"name":"swap","discriminator":[1],"accounts":[],"args":[]},
    {"name":"swap","discriminator":[2],"accounts":[],"args":[]}
  ]
}"#;

#[test]
fn duplicate_instruction_name_is_flagged() {
    let idl = parse(DUP);
    let r = diff(&idl, &idl);
    assert!(
        r.changes
            .iter()
            .any(|c| c.message.contains("duplicate instruction name")),
        "expected a duplicate-name finding; got {:?}",
        r.changes
    );
}

// KEYSTONE: default-omission normalization. Two IDLs that are semantically
// identical but differ only in whether default fields are written explicitly
// MUST diff to zero changes. This is the property serde's `#[serde(default)]`
// gives us; if it ever regresses, the whole tool floods false positives.
const OMITTED: &str = r#"{
  "address":"A","metadata":{"name":"o","version":"1","spec":"0.1.0"},
  "instructions":[{"name":"swap","discriminator":[1],
    "accounts":[{"name":"pool","writable":true}]}]
}"#;
// Same meaning, but every default written explicitly: writable:false, signer:false,
// optional:false, empty args:[], empty docs, borsh serialization, etc.
const EXPLICIT: &str = r#"{
  "address":"A","metadata":{"name":"o","version":"1","spec":"0.1.0"},
  "instructions":[{"name":"swap","discriminator":[1],
    "accounts":[{"name":"pool","writable":true,"signer":false,"optional":false}],
    "args":[]}],
  "accounts":[],"events":[],"errors":[],"types":[]
}"#;

#[test]
fn default_omission_produces_no_changes() {
    let r = diff(&parse(OMITTED), &parse(EXPLICIT));
    assert!(
        r.changes.is_empty(),
        "default-omission must normalize to zero changes; got {:?}",
        r.changes
    );
    assert_eq!(r.exit_code(), 0);
}

/// Real Codama `RootNode` JSON (same shape Vixen's `include_vixen_parser!` loads).
/// Self-diff must be empty — proves Codama deserialize + normalize is stable.
#[test]
fn codama_order_engine_self_diff_is_empty() {
    let src = include_str!("fixtures/codama_order_engine.json");
    let idl = parse(src);
    assert_eq!(idl.metadata.name, "orderEngine");
    assert_eq!(idl.address, "61DFfeTKM7trxYcPQCM78bJ794ddZprZpAwAnLiwTpYH");
    assert_eq!(idl.instructions.len(), 1);
    assert_eq!(idl.instructions[0].name, "fill");
    // fieldDiscriminator → bytes from omitted discriminator arg
    assert_eq!(
        idl.instructions[0].discriminator,
        vec![0xa8, 0x60, 0xb7, 0xa3, 0x5c, 0x0a, 0x28, 0xa0]
    );
    // discriminator arg itself is omitted from args[]
    assert_eq!(idl.instructions[0].args.len(), 3);

    let r = diff(&idl, &idl);
    assert!(
        r.changes.is_empty(),
        "Codama self-diff must be empty; got {:?}",
        r.changes
    );
    assert_eq!(r.exit_code(), 0);
}

/// Codama sample with a top-level `program.events` entry — confirms events are
/// normalized into `Idl.events` with discriminator bytes (Vixen's separate list).
#[test]
fn codama_events_are_top_level_with_discriminator() {
    let idl = parse(include_str!("fixtures/codama_tiny.json"));
    assert_eq!(idl.events.len(), 1);
    assert_eq!(idl.events[0].name, "pong");
    assert_eq!(
        idl.events[0].discriminator,
        vec![0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x11]
    );
    let r = diff(&idl, &idl);
    assert!(r.changes.is_empty(), "got {:?}", r.changes);
}

// Self-CPI event envelope: Anchor uses an 8-byte event-ix tag; Pinocchio often
// uses a 1-byte custom envelope (Vixen `cpi_event_discriminator` /
// `cpi_event_payload_offset`). A length change is a scheme swap → Breaking.
const EVENT_ENV_8: &str = r#"{
  "address":"A","metadata":{"name":"e","version":"1","spec":"0.1.0"},
  "instructions":[{"name":"noop","discriminator":[1],"accounts":[],"args":[]}],
  "events":[{
    "name":"Trade",
    "discriminator":[1,2,3,4,5,6,7,8],
    "envelope_discriminator":[228,69,165,46,81,203,154,29],
    "envelope_payload_offset":8
  }]
}"#;
const EVENT_ENV_1: &str = r#"{
  "address":"A","metadata":{"name":"e","version":"1","spec":"0.1.0"},
  "instructions":[{"name":"noop","discriminator":[1],"accounts":[],"args":[]}],
  "events":[{
    "name":"Trade",
    "discriminator":[1,2,3,4,5,6,7,8],
    "envelope_discriminator":[254],
    "envelope_payload_offset":1
  }]
}"#;

#[test]
fn event_envelope_length_8_to_1_is_breaking() {
    let r = diff(&parse(EVENT_ENV_8), &parse(EVENT_ENV_1));
    assert!(
        r.has_breaking(),
        "8-byte -> 1-byte event envelope must be breaking; got {:?}",
        r.changes
    );
    assert!(
        r.changes.iter().any(|c| {
            c.severity == Severity::Breaking
                && c.message.contains("envelope discriminator LENGTH")
                && c.message.contains("8->1")
        }),
        "expected envelope LENGTH finding; got {:?}",
        r.changes
    );
    assert_eq!(r.exit_code(), 1);
}

// --- Discriminator collision detection (Vixen InstructionParser fan-out) ---

fn bare_idl(instructions: Vec<Instruction>) -> Idl {
    Idl {
        address: "A".into(),
        metadata: Metadata {
            name: "t".into(),
            version: "1".into(),
            spec: "0.1.0".into(),
        },
        instructions,
        accounts: vec![],
        events: vec![],
        errors: vec![],
        types: vec![],
    }
}

fn acct(name: &str) -> AccountItem {
    AccountItem::Single(Account {
        name: name.into(),
        writable: false,
        signer: false,
        optional: false,
    })
}

fn opt_acct(name: &str) -> AccountItem {
    AccountItem::Single(Account {
        name: name.into(),
        writable: false,
        signer: false,
        optional: true,
    })
}

fn ix(name: &str, disc: Vec<u8>, accounts: Vec<AccountItem>) -> Instruction {
    Instruction {
        name: name.into(),
        discriminator: disc,
        accounts,
        args: vec![],
    }
}

#[test]
fn same_disc_same_account_count_is_breaking() {
    let disc = vec![0x01, 0x02, 0x03, 0x04];
    let old = bare_idl(vec![ix("swap_a", disc.clone(), vec![acct("a"), acct("b")])]);
    let new = bare_idl(vec![
        ix("swap_a", disc.clone(), vec![acct("a"), acct("b")]),
        ix("swap_b", disc, vec![acct("c"), acct("d")]),
    ]);
    let r = diff(&old, &new);
    assert!(
        r.has_breaking(),
        "same disc + same account count must be breaking; got {:?}",
        r.changes
    );
    assert!(
        r.changes
            .iter()
            .any(|c| c.message.contains("unresolvable collision")),
        "expected unresolvable collision; got {:?}",
        r.changes
    );
    assert_eq!(r.exit_code(), 1);
}

#[test]
fn same_disc_different_account_count_is_not_breaking() {
    let disc = vec![0x01, 0x02, 0x03, 0x04];
    let idl = bare_idl(vec![
        ix("swap_a", disc.clone(), vec![acct("a"), acct("b")]),
        ix("swap_b", disc, vec![acct("a"), acct("b"), acct("c")]),
    ]);
    let r = diff(&idl, &idl);
    assert!(
        !r.has_breaking(),
        "fan-out by account count is resolvable; got {:?}",
        r.changes
    );
}

#[test]
fn disambiguation_order_change_is_dangerous() {
    let disc = vec![0xaa, 0xbb];
    // Old counts {2,4}; new {2,3}. Shrink via optional-tail removal so the
    // positional account rule stays Dangerous (not Breaking).
    let old = bare_idl(vec![
        ix("swap_a", disc.clone(), vec![acct("a"), acct("b")]),
        ix(
            "swap_b",
            disc.clone(),
            vec![acct("a"), acct("b"), acct("c"), opt_acct("d")],
        ),
    ]);
    let new = bare_idl(vec![
        ix("swap_a", disc.clone(), vec![acct("a"), acct("b")]),
        ix("swap_b", disc, vec![acct("a"), acct("b"), acct("c")]),
    ]);
    let r = diff(&old, &new);
    assert!(
        !r.has_breaking(),
        "order change alone must not be breaking; got {:?}",
        r.changes
    );
    assert!(
        r.changes.iter().any(|c| {
            c.severity == Severity::Dangerous && c.message.contains("disambiguation order changed")
        }),
        "expected disambiguation order Dangerous; got {:?}",
        r.changes
    );
}

// --- Discriminator encoding normalization (base16 / base58 / base64) ---

#[test]
fn same_discriminator_two_encodings_is_no_change() {
    // Same underlying bytes [1,2,3,4] under every Codama encoding.
    let expected = vec![1u8, 2, 3, 4];
    let from_hex = decode_encoded_bytes("base16", "01020304");
    let from_b58 = decode_encoded_bytes("base58", "2VfUX");
    let from_b64 = decode_encoded_bytes("base64", "AQIDBA==");
    assert_eq!(from_hex, expected, "base16");
    assert_eq!(from_b58, expected, "base58");
    assert_eq!(from_b64, expected, "base64");
    assert_eq!(from_hex, from_b58);
    assert_eq!(from_hex, from_b64);

    // Full Codama path: same account constant disc as base16 vs base58 must
    // not produce a false-positive "discriminator changed".
    let base16_idl = r#"{
      "kind":"rootNode","standard":"codama","version":"1.5.1",
      "program":{
        "kind":"programNode","name":"enc","publicKey":"11111111111111111111111111111111","version":"0.1.0",
        "accounts":[{
          "kind":"accountNode","name":"vault",
          "data":{"kind":"structTypeNode","fields":[
            {"kind":"structFieldTypeNode","name":"discriminator","defaultValueStrategy":"omitted",
             "type":{"kind":"fixedSizeTypeNode","size":4,"type":{"kind":"bytesTypeNode"}},
             "defaultValue":{"kind":"bytesValueNode","data":"01020304","encoding":"base16"}},
            {"kind":"structFieldTypeNode","name":"bal",
             "type":{"kind":"numberTypeNode","format":"u64","endian":"le"}}
          ]},
          "discriminators":[{"kind":"fieldDiscriminatorNode","name":"discriminator","offset":0}]
        }],
        "instructions":[],"definedTypes":[],"pdas":[],"events":[],"errors":[]
      },
      "additionalPrograms":[]
    }"#;
    let base58_idl = r#"{
      "kind":"rootNode","standard":"codama","version":"1.5.1",
      "program":{
        "kind":"programNode","name":"enc","publicKey":"11111111111111111111111111111111","version":"0.1.0",
        "accounts":[{
          "kind":"accountNode","name":"vault",
          "data":{"kind":"structTypeNode","fields":[
            {"kind":"structFieldTypeNode","name":"discriminator","defaultValueStrategy":"omitted",
             "type":{"kind":"fixedSizeTypeNode","size":4,"type":{"kind":"bytesTypeNode"}},
             "defaultValue":{"kind":"bytesValueNode","data":"2VfUX","encoding":"base58"}},
            {"kind":"structFieldTypeNode","name":"bal",
             "type":{"kind":"numberTypeNode","format":"u64","endian":"le"}}
          ]},
          "discriminators":[{"kind":"fieldDiscriminatorNode","name":"discriminator","offset":0}]
        }],
        "instructions":[],"definedTypes":[],"pdas":[],"events":[],"errors":[]
      },
      "additionalPrograms":[]
    }"#;

    let a = parse(base16_idl);
    let b = parse(base58_idl);
    assert_eq!(a.accounts[0].discriminator, expected);
    assert_eq!(b.accounts[0].discriminator, expected);

    let r = diff(&a, &b);
    assert!(
        !r.has_breaking(),
        "same disc under base16 vs base58 must not break; got {:?}",
        r.changes
    );
    assert!(
        !r.changes
            .iter()
            .any(|c| c.message.contains("discriminator")),
        "zero discriminator-change entries expected; got {:?}",
        r.changes
    );
}

// --- Event-payload reachability (Codama PR #985: events de-dupe definedTypes) ---

fn typedef_struct(name: &str, fields: Vec<Field>) -> TypeDef {
    TypeDef {
        name: name.into(),
        serialization: "borsh".into(),
        ty: TypeDefTy::Struct { fields },
    }
}

fn field(name: &str, ty: Type) -> Field {
    Field {
        name: name.into(),
        ty,
    }
}

fn defined(name: &str) -> Type {
    Type::Defined {
        name: name.into(),
        generics: vec![],
    }
}

fn event_named(name: &str) -> Event {
    Event {
        name: name.into(),
        discriminator: vec![1, 2, 3, 4, 5, 6, 7, 8],
        envelope_discriminator: vec![],
        envelope_payload_offset: None,
    }
}

fn idl_with(events: Vec<Event>, types: Vec<TypeDef>) -> Idl {
    Idl {
        address: "A".into(),
        metadata: Metadata {
            name: "t".into(),
            version: "1".into(),
            spec: "0.1.0".into(),
        },
        instructions: vec![],
        accounts: vec![],
        events,
        errors: vec![],
        types,
    }
}

#[test]
fn event_only_type_field_change_is_breaking() {
    // Trade event → Trade payload struct → Defined(TradeDetail). Nothing else
    // references TradeDetail. Reachability must still pull it in via the event
    // name root + transitive struct-field closure.
    let old = idl_with(
        vec![event_named("Trade")],
        vec![
            typedef_struct("Trade", vec![field("detail", defined("TradeDetail"))]),
            typedef_struct("TradeDetail", vec![field("px", Type::U16)]),
        ],
    );
    let new = idl_with(
        vec![event_named("Trade")],
        vec![
            typedef_struct("Trade", vec![field("detail", defined("TradeDetail"))]),
            typedef_struct("TradeDetail", vec![field("px", Type::U32)]),
        ],
    );
    let r = diff(&old, &new);
    assert!(
        r.has_breaking(),
        "event-only Defined type field change must be Breaking; got {:?}",
        r.changes
    );
    assert!(
        r.changes.iter().any(|c| {
            c.severity == Severity::Breaking
                && c.message.contains("TradeDetail")
                && c.message.contains("Borsh layout")
        }),
        "expected Breaking TradeDetail Borsh layout; got {:?}",
        r.changes
    );
    assert!(
        !r.changes.iter().any(|c| c.message.contains("UNREACHED")),
        "must not be Cosmetic/UNREACHED; got {:?}",
        r.changes
    );
    assert_eq!(r.exit_code(), 1);
}

#[test]
fn truly_dead_type_change_is_cosmetic() {
    // Trade event payload does NOT reference TradeDetail; nothing else does
    // either. U16→U32 on TradeDetail must stay Cosmetic.
    let old = idl_with(
        vec![event_named("Trade")],
        vec![
            typedef_struct("Trade", vec![field("qty", Type::U64)]),
            typedef_struct("TradeDetail", vec![field("px", Type::U16)]),
        ],
    );
    let new = idl_with(
        vec![event_named("Trade")],
        vec![
            typedef_struct("Trade", vec![field("qty", Type::U64)]),
            typedef_struct("TradeDetail", vec![field("px", Type::U32)]),
        ],
    );
    let r = diff(&old, &new);
    assert!(
        !r.has_breaking(),
        "truly dead type must not be Breaking; got {:?}",
        r.changes
    );
    assert!(
        r.changes
            .iter()
            .any(|c| { c.severity == Severity::Cosmetic && c.message.contains("UNREACHED") }),
        "expected Cosmetic UNREACHED; got {:?}",
        r.changes
    );
}

// --- Map / Set / Tuple structural layout (Borsh-bearing composites) ---

fn idl_account(account: &str, types: Vec<TypeDef>) -> Idl {
    Idl {
        address: "A".into(),
        metadata: Metadata {
            name: "t".into(),
            version: "1".into(),
            spec: "0.1.0".into(),
        },
        instructions: vec![],
        accounts: vec![NamedDisc {
            name: account.into(),
            discriminator: vec![1, 2, 3, 4, 5, 6, 7, 8],
        }],
        events: vec![],
        errors: vec![],
        types,
    }
}

#[test]
fn map_value_widen_on_reachable_field_is_breaking() {
    let old = idl_account(
        "Vault",
        vec![typedef_struct(
            "Vault",
            vec![field(
                "balances",
                Type::Map(Box::new(Type::Pubkey), Box::new(Type::U64)),
            )],
        )],
    );
    let new = idl_account(
        "Vault",
        vec![typedef_struct(
            "Vault",
            vec![field(
                "balances",
                Type::Map(Box::new(Type::Pubkey), Box::new(Type::U128)),
            )],
        )],
    );
    let r = diff(&old, &new);
    assert!(
        r.has_breaking(),
        "map value widen must Breaking; got {:?}",
        r.changes
    );
    assert!(
        r.changes.iter().any(|c| {
            c.severity == Severity::Breaking
                && (c.message.contains("balances")
                    || c.message.contains("Vault#0")
                    || c.message.contains("Map:"))
        }),
        "expected Breaking mentioning the balances field; got {:?}",
        r.changes
    );
}

#[test]
fn set_element_change_is_breaking() {
    let old = idl_account(
        "Book",
        vec![typedef_struct(
            "Book",
            vec![field("ids", Type::Set(Box::new(Type::U32)))],
        )],
    );
    let new = idl_account(
        "Book",
        vec![typedef_struct(
            "Book",
            vec![field("ids", Type::Set(Box::new(Type::U64)))],
        )],
    );
    let r = diff(&old, &new);
    assert!(
        r.has_breaking(),
        "set element change must Breaking; got {:?}",
        r.changes
    );
}

#[test]
fn tuple_element_reorder_is_breaking() {
    let old = idl_account(
        "Pair",
        vec![typedef_struct(
            "Pair",
            vec![field("xy", Type::Tuple(vec![Type::U8, Type::U64]))],
        )],
    );
    let new = idl_account(
        "Pair",
        vec![typedef_struct(
            "Pair",
            vec![field("xy", Type::Tuple(vec![Type::U64, Type::U8]))],
        )],
    );
    let r = diff(&old, &new);
    assert!(
        r.has_breaking(),
        "tuple element reorder must Breaking; got {:?}",
        r.changes
    );
}

#[test]
fn map_defined_value_stays_reachable() {
    // Slot is referenced ONLY as Map value inside reachable Vault — must not
    // be treated as dead/Cosmetic when its field widens.
    let old = idl_account(
        "Vault",
        vec![
            typedef_struct(
                "Vault",
                vec![field(
                    "bySlot",
                    Type::Map(Box::new(Type::U64), Box::new(defined("Slot"))),
                )],
            ),
            typedef_struct("Slot", vec![field("lamports", Type::U16)]),
        ],
    );
    let new = idl_account(
        "Vault",
        vec![
            typedef_struct(
                "Vault",
                vec![field(
                    "bySlot",
                    Type::Map(Box::new(Type::U64), Box::new(defined("Slot"))),
                )],
            ),
            typedef_struct("Slot", vec![field("lamports", Type::U32)]),
        ],
    );
    let r = diff(&old, &new);
    assert!(
        r.has_breaking(),
        "Defined-only-via-Map-value must stay reachable/Breaking; got {:?}",
        r.changes
    );
    assert!(
        !r.changes.iter().any(|c| c.message.contains("UNREACHED")),
        "must not be Cosmetic/UNREACHED; got {:?}",
        r.changes
    );
}

#[test]
fn unchanged_map_set_tuple_is_no_change() {
    let fields = vec![
        field(
            "balances",
            Type::Map(Box::new(Type::Pubkey), Box::new(Type::U64)),
        ),
        field("ids", Type::Set(Box::new(Type::U32))),
        field("xy", Type::Tuple(vec![Type::U8, Type::U64])),
    ];
    let old = idl_account("Vault", vec![typedef_struct("Vault", fields.clone())]);
    let new = idl_account("Vault", vec![typedef_struct("Vault", fields)]);
    let r = diff(&old, &new);
    assert!(
        r.changes.is_empty(),
        "unchanged Map/Set/Tuple must be zero changes; got {:?}",
        r.changes
    );
}

// --- Real Vixen Codama: limit_order v1 -> v2 ---

fn limit_order_v1() -> Idl {
    parse(include_str!("idls/limit_order_v1.json"))
}

fn limit_order_v2() -> Idl {
    parse(include_str!("idls/limit_order_v2.json"))
}

#[test]
fn limit_order_v1_to_v2_is_breaking() {
    let old = limit_order_v1();
    let new = limit_order_v2();
    let r = diff(&old, &new);

    assert!(
        r.has_breaking(),
        "v1->v2 must be Breaking; got {:?}",
        r.changes
    );
    assert_eq!(r.exit_code(), 1);

    // Fee account: four u64 fees -> two u16 bps (+ removals).
    assert!(
        r.changes.iter().any(|c| {
            c.severity == Severity::Breaking
                && c.message.contains("fee#")
                && (c.message.contains("u64 -> u16") || c.message.contains("present -> removed"))
        }),
        "expected fee account layout Breaking; got {:?}",
        r.changes
            .iter()
            .filter(|c| c.message.contains("fee"))
            .collect::<Vec<_>>()
    );

    // Bare structTypeNode events (v1) -> hiddenPrefix CPI envelope (v2).
    assert!(
        r.changes.iter().any(|c| {
            c.severity == Severity::Breaking && c.message.to_lowercase().contains("envelope")
        }),
        "expected event-envelope Breaking for bare->CPI-wrapped; got {:?}",
        r.changes
    );

    assert!(
        r.changes.iter().any(|c| {
            c.severity == Severity::Breaking && c.message.contains("instruction removed: initFee")
        }),
        "expected initFee removal; got {:?}",
        r.changes
    );
    assert!(
        r.changes.iter().any(|c| {
            c.severity == Severity::Breaking
                && c.message
                    .contains("instruction removed: cancelExpiredOrder")
        }),
        "expected cancelExpiredOrder removal; got {:?}",
        r.changes
    );

    assert!(
        r.changes
            .iter()
            .any(|c| { c.severity == Severity::Additive && c.message.contains("cancelDustOrder") }),
        "expected Additive cancelDustOrder; got {:?}",
        r.changes
    );
}

#[test]
fn limit_order_idls_have_no_unexpected_generics() {
    use idl_drift::coverage;
    use idl_drift::model::Type;

    for (label, idl) in [("v1", limit_order_v1()), ("v2", limit_order_v2())] {
        let report = coverage(&idl);
        assert!(
            report.fully_mapped(),
            "{label}: unexpected reachable Generic: {:?}",
            report.unmapped
        );
    }

    // v2 fillOrder.swapData: sizePrefixTypeNode(bytes, u32) -> Type::Bytes.
    let v2 = limit_order_v2();
    let fill = v2
        .instructions
        .iter()
        .find(|i| i.name == "fillOrder")
        .expect("fillOrder");
    let swap = fill
        .args
        .iter()
        .find(|a| a.name == "swapData")
        .expect("swapData arg");
    assert_eq!(
        swap.ty,
        Type::Bytes,
        "swapData must map to Bytes, not Generic; got {:?}",
        swap.ty
    );
}
