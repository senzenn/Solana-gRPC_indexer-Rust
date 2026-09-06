# account_removed_collision

## Real-world situation
Two instructions share a discriminator (Vixen fan-out by account count). In v1,
`place` has 3 accounts and `cancel` has 2 — distinguishable. v2 removes the
non-tail `oracle` account from `place`, so both now have the same disc **and**
the same account count. Vixen’s `InstructionParser` can no longer disambiguate
without a hand-written resolver.

## Expected
- Classification: **Breaking** containing `unresolvable collision`
- Exit code: **1**
- (May also report Breaking for the removed account / position shift.)
