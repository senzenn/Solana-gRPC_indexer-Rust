#!/usr/bin/awk -f
# diff.awk — the idl-drift diff/classify engine (awk prototype).
#
# Usage:  awk -f diff.awk OLD.facts NEW.facts
#
# Reads two flattened fact files (TYPE|KEY|VALUE per line) and reports each
# change classified as BREAKING / DANGEROUS / ADDITIVE / COSMETIC.
# Exit code: 1 if any BREAKING change, else 0  (the oasdiff CI contract).
#
# This is the faithful awk stand-in for the Rust normalize->diff->classify.
# The classification rules encode everything the research established:
#   - Borsh is positional: any struct FIELD change = BREAKING
#   - accounts are positional: new/removed/reordered required account = BREAKING
#   - discriminator change = BREAKING; disc length change = BREAKING (scheme swap)
#   - new instruction/account/event = ADDITIVE
#   - a purely-trailing new account = DANGEROUS (may or may not break callers)

BEGIN { FS="|"; nbreaking=0; ndanger=0; nadd=0; ncos=0 }

# ---- pass 1: load OLD (first file) ----
FNR==NR {
    old[$1"|"$2] = $3;
    oldkeys[$1"|"$2] = 1;
    if ($1=="IX")   oldParent["IX|" $2] = 1;
    if ($1=="TYPE") { oldParent["TYPE|" $2] = 1; oldKind[$2] = kindOf($3) }
    # track the highest variant index per enum, both sides, to tell
    # append (safe-ish) from middle-insert (index shift). Blocker #6.
    if ($1=="VARIANT") { en=$2; pp=en; sub(/#.*/,"",pp); idx=en; sub(/.*#/,"",idx);
                         if (idx+0 > oldMaxVar[pp]+0) oldMaxVar[pp]=idx+0; oldVarCount[pp]++ }
    # track highest account index per instruction (for tail-removal check, #9)
    if ($1=="IXACCT") { ixn=$2; sub(/#.*/,"",ixn); ai=$2; sub(/.*#/,"",ai);
                        if (ai+0 > oldMaxAcct[ixn]+0) oldMaxAcct[ixn]=ai+0 }
    next;
}

# ---- pass 2a: on the NEW file, first record which parents exist on each side,
#      so we can tell "child of a newly-added parent" from "child added to an
#      existing parent". (Blocker #3 fix: parent add/remove subsumes children.)
{
    if ($1=="IX")   newParent["IX|" $2] = 1;
    if ($1=="TYPE") newParent["TYPE|" $2] = 1;
    all[NR] = $0;   # buffer NEW lines for a second pass in END
}

# ---- pass 2b: walk buffered NEW lines and compare ----
FNR!=NR { }  # (no-op; real work in END so parent maps are fully built first)

END {
    # ---- reachability pass (Blocker #4 fix) ----
    # A type is "reached" if some instruction arg/account or another reached
    # type's field references it via Defined:<Name>. Field changes in types
    # that are NOT reached are dead code -> not breaking.
    # Seed: every Defined:X referenced by an IXARG (instruction-level) is a root.
    for (i = 1; i <= NR; i++) {
        if (!(i in all)) continue;
        split(all[i], f, "|");
        # Root 1: an instruction arg typed Defined:X makes X reachable.
        if (f[1]=="IXARG") {
            r = definedRef(f[3]);
            if (r != "") reached[r] = 1;
        }
        # Root 2 (Blocker #5 fix): an account/event entry names a type that is
        # parsed directly, so that TYPE is inherently reachable — by NAME.
        if (f[1]=="ACCT" || f[1]=="EVENT") {
            reached[f[2]] = 1;
        }
    }
    # Transitive closure: fields of reached types may reference more types.
    changed = 1;
    while (changed) {
        changed = 0;
        for (i = 1; i <= NR; i++) {
            if (!(i in all)) continue;
            split(all[i], f, "|");
            if (f[1]=="FIELD") {
                owner = f[2]; sub(/#.*/, "", owner);
                if (owner in reached) {
                    r = definedRef(f[3]);
                    if (r != "" && !(r in reached)) { reached[r] = 1; changed = 1 }
                }
            }
        }
    }

    # precompute kind changes (Blocker #16): if a type's kind flipped
    # (struct<->enum<->type), the "type kind changed" finding subsumes its
    # child field/variant diffs — suppress those to avoid noise.
    for (i = 1; i <= NR; i++) {
        if (!(i in all)) continue;
        split(all[i], f, "|");
        if (f[1]=="TYPE") {
            nk = kindOf(f[3]);
            if ((f[2] in oldKind) && oldKind[f[2]] != nk) kindChanged[f[2]] = 1;
        }
    }

    # pass 2b: compare each buffered NEW line against OLD, parent-aware.
    for (i = 1; i <= NR; i++) {
        if (!(i in all)) continue;
        n = split(all[i], f, "|");
        if (n < 2) continue;
        kind = f[1]; nm = f[2]; val = f[3];
        key = kind "|" nm;
        newkeys[key] = 1;

        # Blocker #16: skip child field/variant diffs when the owning type's
        # kind changed (the kind-change finding already covers it).
        if (kind=="FIELD" || kind=="VARIANT") {
            owner = nm; sub(/#.*/, "", owner);
            if (owner in kindChanged) continue;
        }

        # Blocker #3 fix: if this element is a CHILD of a parent that is newly
        # added (exists in NEW, not in OLD), skip it — the parent's ADDITIVE
        # finding already covers it. Only report children of parents present
        # on BOTH sides (i.e. genuine drift on an existing instruction/type).
        parent = parentOf(kind, nm);
        if (parent != "" && (parent in newParent) && !(parent in oldParent)) {
            continue;   # child of a newly-added parent — subsumed
        }

        if (!(key in oldkeys)) {
            classifyAdd(kind, nm, val);
        } else if (old[key] != val) {
            classifyChange(kind, nm, old[key], val);
        }
    }

    # removals: in OLD but not NEW — also parent-aware
    for (k in oldkeys) {
        if (!(k in newkeys)) {
            split(k, p, "|");
            parent = parentOf(p[1], p[2]);
            if (parent != "" && (parent in oldParent) && !(parent in newParent)) {
                continue;   # child of a removed parent — subsumed
            }
            # Blocker #16: child of a kind-changed type — subsumed by the
            # "type kind changed" finding.
            if (p[1]=="FIELD" || p[1]=="VARIANT") {
                ow = p[2]; sub(/#.*/, "", ow);
                if (ow in kindChanged) continue;
            }
            classifyRemove(p[1], p[2], old[k]);
        }
    }
    print "";
    printf("SUMMARY: %d breaking, %d dangerous, %d additive, %d cosmetic\n",
           nbreaking, ndanger, nadd, ncos);
    if (nbreaking > 0) { print "RESULT: FAIL (breaking changes present)"; exit 1 }
    else { print "RESULT: PASS"; exit 0 }
}

# Given a child element, return its parent key ("IX|<name>" or "TYPE|<name>"),
# or "" if the element is itself top-level.
function parentOf(kind, nm,   base) {
    if (kind=="IXACCT" || kind=="IXARG") { base=nm; sub(/#.*/, "", base); return "IX|" base }
    if (kind=="FIELD" || kind=="VARIANT") { base=nm; sub(/#.*/, "", base); return "TYPE|" base }
    return "";
}

# ------- classification rules -------

function classifyAdd(kind, name, val) {
    if (kind=="IX")     { report("ADDITIVE", "new instruction: " name); nadd++; return }
    if (kind=="ACCT")   { report("ADDITIVE", "new account type: " name); nadd++; return }
    if (kind=="EVENT")  { report("ADDITIVE", "new event type: " name); nadd++; return }
    if (kind=="TYPE")   { return }  # type shell add reported via its fields
    if (kind=="IXACCT") {
        # a new positional account on an existing instruction.
        report("DANGEROUS", "new account on instruction (" name "): shifts account count/positions -> Vixen check_min_accounts_req expects more");
        ndanger++; return
    }
    if (kind=="IXARG")  { report("BREAKING", "new arg (" name "): Borsh layout grows"); nbreaking++; return }
    if (kind=="FIELD")  { report("BREAKING", "new struct field (" name "): Borsh layout shifts"); nbreaking++; return }
    if (kind=="VARIANT") {
        en=name; sub(/#.*/,"",en); idx=name; sub(/.*#/,"",idx);
        # reachability: variants of an unreached enum are dead code
        if (!(en in reached)) { report("COSMETIC","variant added to UNREACHED enum ("name"): dead"); ncos++; return }
        if (idx+0 > oldMaxVar[en]+0) {
            report("DANGEROUS", "enum variant APPENDED ("name"): existing tags unchanged, but parser must handle new variant");
            ndanger++;
        } else {
            report("BREAKING", "enum variant INSERTED at non-terminal index ("name"): shifts later variant tags -> silent Borsh misdecode");
            nbreaking++;
        }
        return;
    }
    report("COSMETIC", kind " added: " name); ncos++;
}

function classifyRemove(kind, name, val) {
    if (kind=="IX")     { report("BREAKING", "instruction removed: " name); nbreaking++; return }
    if (kind=="IXACCT") {
        # #9: removing an OPTIONAL account from the TAIL doesn't shift any
        # required account -> DANGEROUS, not BREAKING. Middle removal, or any
        # required account removal, still shifts positions -> BREAKING.
        ixn=name; sub(/#.*/,"",ixn); ai=name; sub(/.*#/,"",ai);
        isOpt = (val ~ /optional=1/);
        isTail = (ai+0 == oldMaxAcct[ixn]+0);
        if (isOpt && isTail) {
            report("DANGEROUS", "optional account removed from tail (" name "): required positions unchanged, but callers passing it break");
            ndanger++; return;
        }
        report("BREAKING", "account removed from instruction (" name "): positions shift");
        nbreaking++; return;
    }
    if (kind=="IXARG")  { report("BREAKING", "arg removed (" name ")"); nbreaking++; return }
    if (kind=="FIELD")  { report("BREAKING", "struct field removed (" name ")"); nbreaking++; return }
    if (kind=="VARIANT"){ report("BREAKING", "enum variant removed (" name "): shifts later tags -> Borsh misdecode"); nbreaking++; return }
    if (kind=="ACCT")   { report("BREAKING", "account type removed: " name); nbreaking++; return }
    if (kind=="EVENT")  { report("BREAKING", "event type removed: " name); nbreaking++; return }
    report("COSMETIC", kind " removed: " name); ncos++;
}

function classifyChange(kind, name, oldv, newv,   od, nd) {
    if (kind=="IX") {
        # discriminator lives in the IX value as disc=...
        od = discOf(oldv); nd = discOf(newv);
        if (od != nd) {
            if (nfields(od) != nfields(nd))
                report("BREAKING", "discriminator LENGTH changed (" name "): " nfields(od) "->" nfields(nd) " bytes (scheme swap, e.g. Anchor<->Quasar)");
            else
                report("BREAKING", "discriminator changed (" name ")");
            nbreaking++;
        }
        return;
    }
    if (kind=="ACCT" || kind=="EVENT") {
        if (discOf(oldv) != discOf(newv)) { report("BREAKING", kind " discriminator changed: " name); nbreaking++ }
        return;
    }
    if (kind=="FIELD") {
        owner = name; sub(/#.*/, "", owner);
        if (!(owner in reached)) {
            report("COSMETIC", "field type changed in UNREACHED type (" name "): " typeOf(oldv) " -> " typeOf(newv) " (dead type, no consumer)");
            ncos++; return;
        }
        report("BREAKING", "FIELD type changed (" name ") via reachable type: " typeOf(oldv) " -> " typeOf(newv) " (Borsh width/layout changes)");
        nbreaking++; return;
    }
    if (kind=="IXARG") {
        report("BREAKING", kind " type changed (" name "): " typeOf(oldv) " -> " typeOf(newv) " (Borsh width/layout changes)");
        nbreaking++; return;
    }
    if (kind=="IXACCT") {
        report("BREAKING", "account changed at fixed position (" name "): " oldv " -> " newv);
        nbreaking++; return;
    }
    if (kind=="VARIANT") {
        en=name; sub(/#.*/,"",en);
        if (!(en in reached)) { report("COSMETIC","variant changed in UNREACHED enum ("name")"); ncos++; return }
        report("BREAKING", "enum variant changed at fixed index ("name"): " oldv " -> " newv " (tag reassigned -> Borsh misdecode)");
        nbreaking++; return;
    }
    if (kind=="TYPE") {
        os = serOf(oldv); ns = serOf(newv);
        if (os != ns) {
            if (name in reached) {
                report("BREAKING", "serialization mode changed (" name "): " os " -> " ns " (byte layout differs: bytemuck adds alignment padding borsh lacks)");
                nbreaking++;
            } else {
                report("COSMETIC", "serialization changed in UNREACHED type (" name "): " os " -> " ns);
                ncos++;
            }
            return;
        }
        # kind change (struct<->enum<->alias) is also a full-layout break
        if (kindOf(oldv) != kindOf(newv)) {
            report("BREAKING", "type kind changed (" name "): " kindOf(oldv) " -> " kindOf(newv));
            nbreaking++; return;
        }
        return;  # no meaningful TYPE-level change
    }
    if (kind=="ALIAS") {
        en=name;
        if (!(en in reached)) { report("COSMETIC","alias changed in UNREACHED type ("name")"); ncos++; return }
        report("BREAKING", "type alias target changed ("name"): " targetOf(oldv) " -> " targetOf(newv) " (Borsh layout)");
        nbreaking++; return;
    }
    report("COSMETIC", kind " changed: " name); ncos++;
}

function report(tier, msg) { printf("[%-9s] %s\n", tier, msg) }
function discOf(v,  a){ if (match(v,/disc=[0-9,]*/)){a=substr(v,RSTART+5,RLENGTH-5)} return a }
function typeOf(v,  a){ if (match(v,/type=[^;]*/)){a=substr(v,RSTART+5,RLENGTH-5)} return a }
function nfields(csv,  a){ return split(csv, a, ",") }
# extract "SwapParams" from a value containing type=Defined:SwapParams
function definedRef(v,  a){ if (match(v,/Defined:[A-Za-z0-9_]+/)){a=substr(v,RSTART+8,RLENGTH-8)} return a }
function serOf(v,  a){ if (match(v,/serialization=[^;]*/)){a=substr(v,RSTART+14,RLENGTH-14)} return a }
function kindOf(v,  a){ if (match(v,/kind=[^;]*/)){a=substr(v,RSTART+5,RLENGTH-5)} return a }
function targetOf(v,  a){ if (match(v,/target=[^;]*/)){a=substr(v,RSTART+7,RLENGTH-7)} return a }
