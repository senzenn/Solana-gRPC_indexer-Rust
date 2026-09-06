# truncated_idl

## Real-world situation
A cron job or hand-copy can truncate JSON mid-file (network blip, partial write).
The tool must fail closed with a parse error — never panic — so CI can treat
exit 2 as “retry / bad artifact”, distinct from Breaking (exit 1).

## Expected
- Exit code: **2** (I/O or parse error on stderr: `error: …`)
- No panic
