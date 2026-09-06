//! diff.rs — semantic IDL diff + classification.
//!
//! Ported rule-for-rule from the awk prototype (`proto/diff.awk`), which was
//! validated against 13 fixtures. Every rule here is grounded in how Vixen's
//! generated parser actually reads bytes (see crates/spl-token-parser):
//!   - accounts read positionally by index (`ix.accounts[0]`, `[1]`, ...)
//!   - `check_min_accounts_req(len, N)` gates every instruction
//!   - discriminator tag-dispatch routes instructions
//!   - enum `oneof` variants are tag-indexed
//!
//! The four tiers and the exit-code contract mirror oasdiff / GraphQL Inspector.

use crate::model::{AccountItem, Field, GenericArg, Idl, Type, TypeDefTy};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Cosmetic,
    Additive,
    Dangerous,
    Breaking,
}

#[derive(Debug, Clone)]
pub struct Change {
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct Report {
    pub changes: Vec<Change>,
}

impl Report {
    fn push(&mut self, severity: Severity, message: impl Into<String>) {
        self.changes.push(Change {
            severity,
            message: message.into(),
        });
    }
    #[must_use]
    pub fn has_breaking(&self) -> bool {
        self.changes
            .iter()
            .any(|c| c.severity == Severity::Breaking)
    }
    /// oasdiff CI contract: nonzero exit iff a breaking change exists.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        i32::from(self.has_breaking())
    }
    #[must_use]
    pub fn counts(&self) -> (usize, usize, usize, usize) {
        let mut b = 0;
        let mut d = 0;
        let mut a = 0;
        let mut c = 0;
        for ch in &self.changes {
            match ch.severity {
                Severity::Breaking => b += 1,
                Severity::Dangerous => d += 1,
                Severity::Additive => a += 1,
                Severity::Cosmetic => c += 1,
            }
        }
        (b, d, a, c)
    }
}

/// Top-level entry: diff two IDLs and produce a classified report.
#[must_use]
pub fn diff(old: &Idl, new: &Idl) -> Report {
    let mut r = Report::default();

    // --- Blocker #4/#5: reachability set over the NEW idl ---
    // Roots: every account/event type name, plus any type referenced by an
    // instruction arg via `Defined`. Then transitive closure through fields.
    let reached = reachable_types(new);

    // --- Blocker #17: duplicate instruction names. Name-keyed matching
    // silently drops all-but-last on collision, hiding drift on the dropped
    // entries. A duplicate name is itself malformed, so flag it explicitly. ---
    report_duplicate_names(&new.instructions, &mut r);

    // --- instructions: match by name (Blocker #3: parents first) ---
    let old_ix: BTreeMap<_, _> = old.instructions.iter().map(|i| (&i.name, i)).collect();
    let new_ix: BTreeMap<_, _> = new.instructions.iter().map(|i| (&i.name, i)).collect();

    for (name, ni) in &new_ix {
        match old_ix.get(name) {
            None => r.push(Severity::Additive, format!("new instruction: {name}")),
            Some(oi) => diff_instruction(oi, ni, &mut r),
        }
    }
    for name in old_ix.keys() {
        if !new_ix.contains_key(name) {
            r.push(Severity::Breaking, format!("instruction removed: {name}"));
        }
    }

    // Vixen InstructionParser: same discriminator + same account count is
    // unresolvable without a hand-written InstructionResolver.
    diff_discriminator_collisions(old, new, &mut r);

    // --- account types: discriminator-only here (Blocker #2); layout via types[] ---
    diff_named_disc("account", &old.accounts, &new.accounts, &mut r);
    // Events: content disc + optional self-CPI envelope (Anchor 8-byte vs Pinocchio 1-byte).
    diff_events(&old.events, &new.events, &mut r);

    // --- types: fields / variants / serialization, gated by reachability ---
    diff_types(old, new, &reached, &mut r);

    r
}

fn diff_named_disc(
    kind: &str,
    old: &[crate::model::NamedDisc],
    new: &[crate::model::NamedDisc],
    r: &mut Report,
) {
    let om: BTreeMap<_, _> = old.iter().map(|a| (&a.name, a)).collect();
    let nm: BTreeMap<_, _> = new.iter().map(|a| (&a.name, a)).collect();
    for (name, na) in &nm {
        match om.get(name) {
            None => r.push(Severity::Additive, format!("new {kind} type: {name}")),
            Some(oa) if oa.discriminator != na.discriminator => {
                let msg = if oa.discriminator.len() == na.discriminator.len() {
                    format!("{kind} discriminator changed ({name})")
                } else {
                    format!(
                        "{kind} discriminator LENGTH changed ({name}): {}->{} bytes (scheme swap, e.g. Anchor<->Quasar)",
                        oa.discriminator.len(), na.discriminator.len()
                    )
                };
                r.push(Severity::Breaking, msg);
            }
            _ => {}
        }
    }
    for name in om.keys() {
        if !nm.contains_key(name) {
            r.push(Severity::Breaking, format!("{kind} type removed: {name}"));
        }
    }
}

/// Diff events: content discriminator (same rules as accounts) plus the
/// optional self-CPI envelope. An envelope length change is a scheme swap
/// (Anchor 8-byte event-ix tag ↔ Pinocchio 1-byte) and breaks Vixen parsing.
fn diff_events(old: &[crate::model::Event], new: &[crate::model::Event], r: &mut Report) {
    let om: BTreeMap<_, _> = old.iter().map(|e| (&e.name, e)).collect();
    let nm: BTreeMap<_, _> = new.iter().map(|e| (&e.name, e)).collect();
    for (name, ne) in &nm {
        match om.get(name) {
            None => r.push(Severity::Additive, format!("new event type: {name}")),
            Some(oe) => {
                if oe.discriminator != ne.discriminator {
                    let msg = if oe.discriminator.len() == ne.discriminator.len() {
                        format!("event discriminator changed ({name})")
                    } else {
                        format!(
                            "event discriminator LENGTH changed ({name}): {}->{} bytes (scheme swap, e.g. Anchor<->Quasar)",
                            oe.discriminator.len(),
                            ne.discriminator.len()
                        )
                    };
                    r.push(Severity::Breaking, msg);
                }
                diff_event_envelope(name, oe, ne, r);
            }
        }
    }
    for name in om.keys() {
        if !nm.contains_key(name) {
            r.push(Severity::Breaking, format!("event type removed: {name}"));
        }
    }
}

fn diff_event_envelope(
    name: &str,
    oe: &crate::model::Event,
    ne: &crate::model::Event,
    r: &mut Report,
) {
    let old_env = &oe.envelope_discriminator;
    let new_env = &ne.envelope_discriminator;
    // Only compare when at least one side models an envelope.
    if old_env.is_empty() && new_env.is_empty() {
        // Still compare explicit payload offsets if both set.
        match (oe.envelope_payload_offset, ne.envelope_payload_offset) {
            (Some(o), Some(n)) if o != n => r.push(
                Severity::Breaking,
                format!(
                    "event envelope payload offset changed ({name}): {o}->{n} (self-CPI parse starts elsewhere)"
                ),
            ),
            _ => {}
        }
        return;
    }
    if old_env != new_env {
        let msg = if old_env.len() == new_env.len() {
            format!("event envelope discriminator changed ({name})")
        } else {
            format!(
                "event envelope discriminator LENGTH changed ({name}): {}->{} bytes (scheme swap, e.g. Anchor 8-byte <-> Pinocchio 1-byte)",
                old_env.len(),
                new_env.len()
            )
        };
        r.push(Severity::Breaking, msg);
    }
    match (oe.effective_payload_offset(), ne.effective_payload_offset()) {
        (Some(o), Some(n)) if o != n => r.push(
            Severity::Breaking,
            format!(
                "event envelope payload offset changed ({name}): {o}->{n} (self-CPI parse starts elsewhere)"
            ),
        ),
        _ => {}
    }
}

fn diff_instruction(
    oi: &crate::model::Instruction,
    ni: &crate::model::Instruction,
    r: &mut Report,
) {
    // discriminator
    if oi.discriminator != ni.discriminator {
        let msg = if oi.discriminator.len() == ni.discriminator.len() {
            format!("discriminator changed ({})", oi.name)
        } else {
            format!(
                "discriminator LENGTH changed ({}): {}->{} bytes (scheme swap)",
                oi.name,
                oi.discriminator.len(),
                ni.discriminator.len()
            )
        };
        r.push(Severity::Breaking, msg);
    }

    // accounts: FLATTEN composites (Blocker #7), then positional diff (#1/#9)
    let ofa = flatten_accounts(&oi.accounts);
    let nfa = flatten_accounts(&ni.accounts);
    let old_tail = ofa.len().saturating_sub(1);
    let max = ofa.len().max(nfa.len());
    for pos in 0..max {
        match (ofa.get(pos), nfa.get(pos)) {
            (Some(o), Some(n)) if o != n => r.push(
                Severity::Breaking,
                format!("account changed at fixed position ({}#{pos}): {o:?} -> {n:?}", oi.name),
            ),
            (Some(_), None) => {
                // removed account at this position
                let o = &ofa[pos];
                if o.optional && pos == old_tail {
                    r.push(Severity::Dangerous, format!(
                        "optional account removed from tail ({}#{pos}): required positions unchanged", oi.name));
                } else {
                    r.push(Severity::Breaking, format!(
                        "account removed from instruction ({}#{pos}): positions shift", oi.name));
                }
            }
            (None, Some(_)) => r.push(
                Severity::Dangerous,
                format!("new account on instruction ({}#{pos}): shifts count/positions -> check_min_accounts_req expects more", oi.name),
            ),
            _ => {}
        }
    }

    // args: positional; any change/add/remove shifts Borsh layout => breaking
    let max_a = oi.args.len().max(ni.args.len());
    for pos in 0..max_a {
        match (oi.args.get(pos), ni.args.get(pos)) {
            (Some(o), Some(n)) if o != n => r.push(
                Severity::Breaking,
                format!(
                    "arg changed ({}#{pos}): {} -> {} (Borsh layout)",
                    oi.name,
                    tystr(&o.ty),
                    tystr(&n.ty)
                ),
            ),
            (Some(_), None) => r.push(
                Severity::Breaking,
                format!("arg removed ({}#{pos})", oi.name),
            ),
            (None, Some(_)) => r.push(
                Severity::Breaking,
                format!("new arg ({}#{pos}): Borsh layout grows", oi.name),
            ),
            _ => {}
        }
    }
}

/// Flatten composite account groups into a single positional leaf list, in
/// declaration order — Blocker #7. A leaf carries (name, optional) for the
/// tail-optional-removal rule.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Leaf {
    name: String,
    writable: bool,
    signer: bool,
    optional: bool,
}

fn flatten_accounts(items: &[AccountItem]) -> Vec<Leaf> {
    let mut out = Vec::new();
    for it in items {
        match it {
            AccountItem::Single(a) => out.push(Leaf {
                name: a.name.clone(),
                writable: a.writable,
                signer: a.signer,
                optional: a.optional,
            }),
            AccountItem::Composite { accounts, .. } => out.extend(flatten_accounts(accounts)),
        }
    }
    out
}

/// Detect discriminator collisions the way Vixen's `InstructionParser` does:
/// variants share a discriminator and are disambiguated by flattened account
/// count (most accounts first). Same disc + same count ⇒ unresolvable without
/// a hand-written `InstructionResolver`.
fn diff_discriminator_collisions(old: &Idl, new: &Idl, r: &mut Report) {
    let old_classes = collision_classes(&old.instructions);
    let new_classes = collision_classes(&new.instructions);

    // Rule 1: unresolvable (disc, count) classes with ≥2 instructions.
    for ((disc, count), names) in &new_classes {
        if names.len() < 2 {
            continue;
        }
        let hex = disc_hex(disc);
        let name_list = names.iter().cloned().collect::<Vec<_>>().join(", ");
        let preexisting = old_classes
            .get(&(disc.clone(), *count))
            .is_some_and(|n| n.len() >= 2);
        if preexisting {
            r.push(
                Severity::Dangerous,
                format!(
                    "pre-existing collision on discriminator {hex} with {count} accounts: [{name_list}]"
                ),
            );
        } else {
            r.push(
                Severity::Breaking,
                format!(
                    "unresolvable collision on discriminator {hex} with {count} accounts: [{name_list}]"
                ),
            );
        }
    }

    // Rule 2: resolvable fan-out whose account-count set shifted.
    let old_counts = account_counts_by_disc(&old.instructions);
    let new_counts = account_counts_by_disc(&new.instructions);
    for (disc, new_set) in &new_counts {
        if new_set.len() < 2 {
            // Need ≥2 distinct counts (fan-out). Same-count ambiguity is rule 1.
            continue;
        }
        let Some(old_set) = old_counts.get(disc) else {
            continue;
        };
        if old_set.len() < 2 {
            continue;
        }
        if old_set != new_set {
            let hex = disc_hex(disc);
            let old_list = counts_list(old_set);
            let new_list = counts_list(new_set);
            r.push(
                Severity::Dangerous,
                format!(
                    "disambiguation order changed for discriminator {hex}: account counts {{{old_list}}} -> {{{new_list}}}"
                ),
            );
        }
    }
}

/// `(discriminator, flattened_account_count) → sorted instruction names`.
fn collision_classes(
    instructions: &[crate::model::Instruction],
) -> BTreeMap<(Vec<u8>, usize), BTreeSet<String>> {
    let mut map: BTreeMap<(Vec<u8>, usize), BTreeSet<String>> = BTreeMap::new();
    for ix in instructions {
        if ix.discriminator.is_empty() {
            continue;
        }
        let count = flatten_accounts(&ix.accounts).len();
        map.entry((ix.discriminator.clone(), count))
            .or_default()
            .insert(ix.name.clone());
    }
    map
}

/// `discriminator → set of flattened account counts` (non-empty discs only).
fn account_counts_by_disc(
    instructions: &[crate::model::Instruction],
) -> BTreeMap<Vec<u8>, BTreeSet<usize>> {
    let mut map: BTreeMap<Vec<u8>, BTreeSet<usize>> = BTreeMap::new();
    for ix in instructions {
        if ix.discriminator.is_empty() {
            continue;
        }
        let count = flatten_accounts(&ix.accounts).len();
        map.entry(ix.discriminator.clone())
            .or_default()
            .insert(count);
    }
    map
}

fn disc_hex(disc: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(disc.len() * 2);
    for b in disc {
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn counts_list(counts: &BTreeSet<usize>) -> String {
    counts
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn diff_types(old: &Idl, new: &Idl, reached: &BTreeSet<String>, r: &mut Report) {
    let om = old.type_map();
    let nm = new.type_map();
    for (name, nt) in &nm {
        let Some(ot) = om.get(name) else { continue }; // added types reported via refs
        let is_reached = reached.contains(name);

        // serialization-mode change (Blocker #10)
        if ot.serialization != nt.serialization {
            if is_reached {
                r.push(Severity::Breaking, format!(
                    "serialization mode changed ({name}): {} -> {} (byte layout differs: bytemuck adds padding borsh lacks)",
                    ot.serialization, nt.serialization));
            } else {
                r.push(
                    Severity::Cosmetic,
                    format!("serialization changed in UNREACHED type ({name})"),
                );
            }
        }

        match (&ot.ty, &nt.ty) {
            (TypeDefTy::Struct { fields: of }, TypeDefTy::Struct { fields: nf }) => {
                diff_fields(name, of, nf, is_reached, r);
            }
            (TypeDefTy::Enum { variants: ov }, TypeDefTy::Enum { variants: nv }) => {
                diff_variants(name, ov, nv, is_reached, r);
            }
            // Type alias target change, e.g. `Amount = u64` -> `Amount = u32`.
            // A Borsh-width break for every consumer of the alias (Blocker #15).
            (TypeDefTy::Type { alias: oa }, TypeDefTy::Type { alias: na }) if oa != na => {
                if is_reached {
                    r.push(
                        Severity::Breaking,
                        format!(
                            "type alias target changed ({name}): {} -> {} (Borsh layout)",
                            tystr(oa),
                            tystr(na)
                        ),
                    );
                } else {
                    r.push(
                        Severity::Cosmetic,
                        format!("alias changed in UNREACHED type ({name})"),
                    );
                }
            }
            (a, b) if std::mem::discriminant(a) != std::mem::discriminant(b) => {
                r.push(Severity::Breaking, format!("type kind changed ({name})"));
            }
            _ => {}
        }
    }
}

fn diff_fields(ty: &str, of: &[Field], nf: &[Field], reached: bool, r: &mut Report) {
    let max = of.len().max(nf.len());
    for pos in 0..max {
        match (of.get(pos), nf.get(pos)) {
            (Some(o), Some(n)) if o != n => {
                classify_field(ty, pos, &tystr(&o.ty), &tystr(&n.ty), reached, r)
            }
            (Some(_), None) => classify_field(ty, pos, "present", "removed", reached, r),
            (None, Some(_)) => classify_field(ty, pos, "absent", "added", reached, r),
            _ => {}
        }
    }
}

fn classify_field(ty: &str, pos: usize, o: &str, n: &str, reached: bool, r: &mut Report) {
    if reached {
        r.push(
            Severity::Breaking,
            format!("FIELD changed ({ty}#{pos}) via reachable type: {o} -> {n} (Borsh layout)"),
        );
    } else {
        r.push(
            Severity::Cosmetic,
            format!("field changed in UNREACHED type ({ty}#{pos}): dead code"),
        );
    }
}

fn diff_variants(
    ty: &str,
    ov: &[crate::model::EnumVariant],
    nv: &[crate::model::EnumVariant],
    reached: bool,
    r: &mut Report,
) {
    let old_max = ov.len().saturating_sub(1);
    let max = ov.len().max(nv.len());
    for pos in 0..max {
        match (ov.get(pos), nv.get(pos)) {
            (Some(o), Some(n)) if o.name != n.name => {
                if reached {
                    r.push(
                        Severity::Breaking,
                        format!(
                            "enum variant changed at index ({ty}#{pos}): {} -> {} (tag reassigned)",
                            o.name, n.name
                        ),
                    );
                } else {
                    r.push(
                        Severity::Cosmetic,
                        format!("variant changed in UNREACHED enum ({ty}#{pos})"),
                    );
                }
            }
            (None, Some(_)) => {
                if !reached {
                    r.push(
                        Severity::Cosmetic,
                        format!("variant added to UNREACHED enum ({ty}#{pos})"),
                    );
                } else if pos > old_max {
                    r.push(Severity::Dangerous, format!("enum variant APPENDED ({ty}#{pos}): existing tags unchanged, parser must handle new variant"));
                } else {
                    r.push(Severity::Breaking, format!("enum variant INSERTED at non-terminal index ({ty}#{pos}): shifts later tags"));
                }
            }
            (Some(_), None) if reached => {
                r.push(
                    Severity::Breaking,
                    format!("enum variant removed ({ty}#{pos}): shifts later tags"),
                );
            }
            _ => {}
        }
    }
}

/// Blocker #4/#5: compute the set of type names reachable from real consumers.
///
/// Public for `inspect` coverage reporting (same roots as the diff engine).
#[must_use]
pub fn reachable_types(idl: &Idl) -> BTreeSet<String> {
    let mut reached = BTreeSet::new();
    // roots: account + event type names are parsed directly
    for a in &idl.accounts {
        reached.insert(a.name.clone());
    }
    for e in &idl.events {
        reached.insert(e.name.clone());
    }
    // roots: instruction args referencing Defined types (incl. generic args)
    for ix in &idl.instructions {
        for arg in &ix.args {
            let mut refs = Vec::new();
            collect_defined_refs(&arg.ty, &mut refs);
            for name in refs {
                reached.insert(name);
            }
        }
    }
    // transitive closure through fields of reached types
    let tm = idl.type_map();
    let mut changed = true;
    while changed {
        changed = false;
        let snapshot: Vec<String> = reached.iter().cloned().collect();
        for tname in snapshot {
            if let Some(td) = tm.get(&tname) {
                if let TypeDefTy::Struct { fields } = &td.ty {
                    for f in fields {
                        let mut refs = Vec::new();
                        collect_defined_refs(&f.ty, &mut refs);
                        for r in refs {
                            if reached.insert(r) {
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
    }
    reached
}

/// Flag any instruction name that appears more than once. Duplicate names make
/// name-keyed matching lossy (the map keeps only one), so drift on the others
/// would be invisible — surface it as a breaking, actionable finding.
fn report_duplicate_names(instructions: &[crate::model::Instruction], r: &mut Report) {
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for ix in instructions {
        *seen.entry(ix.name.as_str()).or_insert(0) += 1;
    }
    for (name, count) in seen {
        if count > 1 {
            r.push(
                Severity::Breaking,
                format!("duplicate instruction name '{name}' appears {count}x — IDL is ambiguous; matching is unreliable"),
            );
        }
    }
}

fn collect_defined_refs(t: &Type, out: &mut Vec<String>) {
    match t {
        Type::Defined { name, generics } => {
            out.push(name.clone());
            // A generic instantiation like `Wrapper<Node>` references Node too.
            for g in generics {
                if let GenericArg::Type { ty } = g {
                    collect_defined_refs(ty, out);
                }
            }
        }
        Type::Option(inner) | Type::Vec(inner) | Type::Array(inner, _) | Type::Set(inner) => {
            collect_defined_refs(inner, out);
        }
        Type::Map(key, value) => {
            collect_defined_refs(key, out);
            collect_defined_refs(value, out);
        }
        Type::Tuple(items) => {
            for item in items {
                collect_defined_refs(item, out);
            }
        }
        _ => {}
    }
}

fn tystr(t: &Type) -> String {
    match t {
        Type::Defined { name, generics } if generics.is_empty() => format!("Defined:{name}"),
        Type::Defined { name, generics } => {
            let args: Vec<String> = generics
                .iter()
                .map(|g| match g {
                    GenericArg::Type { ty } => tystr(ty),
                    GenericArg::Const { value } => value.clone(),
                })
                .collect();
            format!("Defined:{name}<{}>", args.join(","))
        }
        Type::Option(i) => format!("Option:{}", tystr(i)),
        Type::Vec(i) => format!("Vec:{}", tystr(i)),
        Type::Array(i, _) => format!("Array:{}", tystr(i)),
        Type::Map(k, v) => format!("Map:{}->{}", tystr(k), tystr(v)),
        Type::Set(i) => format!("Set:{}", tystr(i)),
        Type::Tuple(items) => {
            let parts: Vec<String> = items.iter().map(tystr).collect();
            format!("Tuple:({})", parts.join(","))
        }
        other => format!("{other:?}").to_lowercase(),
    }
}
