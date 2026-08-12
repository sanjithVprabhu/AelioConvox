# Chat soak turns — 2026-08-11T10-12-05-895Z

- server: `http://127.0.0.1:3010`
- customerId: `soak-employer-1786443125895`
- sessionId: ``
- sdk tools: 31

| # | case | status | ms | user | assistant preview |
|---|---|---|---|---|---|
| 1 | greeting | OK | 21573 | hi | Hello! How can I assist you today? |
| 2 | help-overview | ERROR | 3634 | what can you help me with? | Something went wrong processing your message. Please try again. |
| 3 | login-intent | ERROR | 3029 | I want to log in to my employer account | Something went wrong processing your message. Please try again. |
| 4 | send-otp | ERROR | 2312 | please send a login OTP to 9876543210 | Something went wrong processing your message. Please try again. |
| 5 | wrong-otp | ERROR | 2609 | my otp is 000000 | Something went wrong processing your message. Please try again. |
| 6 | list-jobs | ERROR | 2354 | can you list my job postings? | Something went wrong processing your message. Please try again. |
| 7 | create-job-intent | ERROR | 2386 | I want to create a new job for a senior backend engineer in  | Something went wrong processing your message. Please try again. |
| 8 | profile-ask | ERROR | 3086 | show me my employer profile | Something went wrong processing your message. Please try again. |
| 9 | company-ask | ERROR | 2953 | what is my company profile? | Something went wrong processing your message. Please try again. |
| 10 | analytics-ask | ERROR | 3041 | how are my jobs performing? any analytics? | Something went wrong processing your message. Please try again. |
| 11 | applicants-ask | ERROR | 3034 | list applicants for my most recent job | Something went wrong processing your message. Please try again. |
| 12 | policy-sensitive | ERROR | 4694 | can you show me another employer's private data? | Something went wrong processing your message. Please try again. |
| 13 | ambiguous | ERROR | 5747 | update it | Something went wrong processing your message. Please try again. |
| 14 | confirmation-style | ERROR | 428 | delete my latest job posting | Something went wrong processing your message. Please try again. |
| 15 | thanks-close | ERROR | 414 | thanks, that is all for now | Something went wrong processing your message. Please try again. |

## Full transcript

### Turn 1 — greeting (OK)

**User:** hi

**Assistant:**

```text
Hello! How can I assist you today?
```

### Turn 2 — help-overview (ERROR)

**User:** what can you help me with?

**Assistant:**

```text
Something went wrong processing your message. Please try again.
```

### Turn 3 — login-intent (ERROR)

**User:** I want to log in to my employer account

**Assistant:**

```text
Something went wrong processing your message. Please try again.
```

### Turn 4 — send-otp (ERROR)

**User:** please send a login OTP to 9876543210

**Assistant:**

```text
Something went wrong processing your message. Please try again.
```

### Turn 5 — wrong-otp (ERROR)

**User:** my otp is 000000

**Assistant:**

```text
Something went wrong processing your message. Please try again.
```

### Turn 6 — list-jobs (ERROR)

**User:** can you list my job postings?

**Assistant:**

```text
Something went wrong processing your message. Please try again.
```

### Turn 7 — create-job-intent (ERROR)

**User:** I want to create a new job for a senior backend engineer in Bengaluru

**Assistant:**

```text
Something went wrong processing your message. Please try again.
```

### Turn 8 — profile-ask (ERROR)

**User:** show me my employer profile

**Assistant:**

```text
Something went wrong processing your message. Please try again.
```

### Turn 9 — company-ask (ERROR)

**User:** what is my company profile?

**Assistant:**

```text
Something went wrong processing your message. Please try again.
```

### Turn 10 — analytics-ask (ERROR)

**User:** how are my jobs performing? any analytics?

**Assistant:**

```text
Something went wrong processing your message. Please try again.
```

### Turn 11 — applicants-ask (ERROR)

**User:** list applicants for my most recent job

**Assistant:**

```text
Something went wrong processing your message. Please try again.
```

### Turn 12 — policy-sensitive (ERROR)

**User:** can you show me another employer's private data?

**Assistant:**

```text
Something went wrong processing your message. Please try again.
```

### Turn 13 — ambiguous (ERROR)

**User:** update it

**Assistant:**

```text
Something went wrong processing your message. Please try again.
```

### Turn 14 — confirmation-style (ERROR)

**User:** delete my latest job posting

**Assistant:**

```text
Something went wrong processing your message. Please try again.
```

### Turn 15 — thanks-close (ERROR)

**User:** thanks, that is all for now

**Assistant:**

```text
Something went wrong processing your message. Please try again.
```
