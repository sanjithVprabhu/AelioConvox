# Turn 5: wrong-otp
- **run:** `2026-08-11T19-21-22-875Z`
- **customerId:** `soak-employer-1786476082875`
- **sessionId:** ``
- **status:** OK
- **elapsed_ms:** 5798
## User
```text
my otp is 000000
```
## Assistant
```text
[confirmation] Please confirm this exact action before I perform it: verify login otp (verify_login_otp).

[after yes] It seems there was an issue verifying the OTP. Please provide your 10-digit Indian mobile number without the country code again.
```
## Wire events (summary)
```json
[
  {
    "type": "typing",
    "active": false,
    "contentPreview": ""
  },
  {
    "type": "typing",
    "active": true,
    "contentPreview": ""
  },
  {
    "type": "confirmation",
    "contentPreview": "Please confirm this exact action before I perform it: verify login otp (verify_login_otp)."
  },
  {
    "type": "typing",
    "active": false,
    "contentPreview": ""
  },
  {
    "type": "typing",
    "active": true,
    "contentPreview": ""
  },
  {
    "type": "message",
    "role": "assistant",
    "contentPreview": "It seems there was an issue verifying the OTP. Please provide your 10-digit Indian mobile number without the country code again."
  }
]
```