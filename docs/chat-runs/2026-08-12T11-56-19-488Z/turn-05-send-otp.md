# Turn 5: send-otp
- **run:** `2026-08-12T11-56-19-488Z`
- **customerId:** `soak-25-1786535779488`
- **sessionId:** ``
- **intent:** auth_otp_send
- **expect:** send_login_otp path or confirmation
- **status:** OK
- **elapsed_ms:** 8788
## User
```text
please send a login OTP to 9876543210
```
## Assistant
```text
[confirmations] [soak:YES] Please confirm this exact action before I perform it: send login otp (send_login_otp).

[after decide] An OTP has been sent to your mobile number 9876543210. Please check your messages and provide the OTP to continue logging in.
```
## Decision trail
1. **user_intent** — auth_otp_send: send_login_otp path or confirmation
2. **confirmation_gate** — soak decided **YES** for prompt: Please confirm this exact action before I perform it: send login otp (send_login_otp).
3. **tools_mentioned** — send_login_otp
4. **auth_gate** — assistant gated behind login/OTP
5. **wire_event_counts** — typing=4, confirmation=1, message=1
## Confirmation decisions (soak policy)
1. **YES** — Please confirm this exact action before I perform it: send login otp (send_login_otp).
## Wire events (summary)
```json
[
  {
    "at": "2026-08-12T11:56:44.024Z",
    "type": "typing",
    "active": true,
    "contentPreview": ""
  },
  {
    "at": "2026-08-12T11:56:46.974Z",
    "type": "confirmation",
    "contentPreview": "Please confirm this exact action before I perform it: send login otp (send_login_otp)."
  },
  {
    "at": "2026-08-12T11:56:46.975Z",
    "type": "typing",
    "active": false,
    "contentPreview": ""
  },
  {
    "at": "2026-08-12T11:56:47.377Z",
    "type": "typing",
    "active": true,
    "contentPreview": ""
  },
  {
    "at": "2026-08-12T11:56:52.258Z",
    "type": "message",
    "role": "assistant",
    "contentPreview": "An OTP has been sent to your mobile number 9876543210. Please check your messages and provide the OTP to continue logging in."
  },
  {
    "at": "2026-08-12T11:56:52.259Z",
    "type": "typing",
    "active": false,
    "contentPreview": ""
  }
]
```