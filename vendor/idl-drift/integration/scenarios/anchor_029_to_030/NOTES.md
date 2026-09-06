# anchor_029_to_030

## Real-world situation
Anchor 0.29 IDLs often omitted explicit 8-byte instruction/account discriminators
(dispatch was by convention / sighash computed client-side). Anchor 0.30+ emits
them in the IDL. Integrators and generated parsers that previously assumed
“no disc bytes in the IDL” suddenly see an 8-byte scheme — a silent layout /
dispatch contract change for anyone hashing from the IDL alone.

## Expected
- Classification: **Breaking** (discriminator LENGTH 0→8 on instruction and/or account)
- Exit code: **1**
