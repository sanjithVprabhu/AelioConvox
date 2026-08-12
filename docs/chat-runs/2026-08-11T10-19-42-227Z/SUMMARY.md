# Chat soak summary — 2026-08-11T10-19-42-227Z

- **result:** FAIL (6/15)
- **server:** `http://127.0.0.1:3010`
- **customerId:** `soak-employer-1786443582227`
- **sessionId:** ``
- **sdk connected:** true
- **tools/states/flows/policies:** 31/4/3/3

## Failures

- turn 4 (send-otp): Timed out waiting for assistant reply for turn 4 (180000ms)
- turn 5 (wrong-otp): Timed out waiting for assistant reply for turn 5 (180000ms)
- turn 8 (profile-ask): Timed out waiting for assistant reply for turn 8 (180000ms)
- turn 9 (company-ask): Timed out waiting for assistant reply for turn 9 (180000ms)
- turn 11 (applicants-ask): Timed out waiting for assistant reply for turn 11 (180000ms)
- turn 12 (policy-sensitive): Timed out waiting for assistant reply for turn 12 (180000ms)

## Per-turn files

- `turn-01-greeting.md`
- `turn-02-help-overview.md`
- `turn-03-login-intent.md`
- `turn-04-send-otp.md`
- `turn-05-wrong-otp.md`
- `turn-06-list-jobs.md`
- `turn-07-create-job-intent.md`
- `turn-08-profile-ask.md`
- `turn-09-company-ask.md`
- `turn-10-analytics-ask.md`
- `turn-11-applicants-ask.md`
- `turn-12-policy-sensitive.md`
- `turn-13-ambiguous.md`
- `turn-14-confirmation-style.md`
- `turn-15-thanks-close.md`

See also `turns.md` for the full transcript.
