# mid_optional_account_removed

## Real-world situation
An optional account (`referral`) sits in the **middle** of the account metas,
not at the tail. Removing it shifts every later fixed position — callers that
indexed by position (and Vixen’s flattened account list) mis-bind remaining
accounts. Contrast: removing a **tail** optional is only Dangerous (required
prefix unchanged).

## Expected
- Classification: **Breaking** (`account removed` / positions shift, and/or
  subsequent `account changed at fixed position`)
- Exit code: **1**
