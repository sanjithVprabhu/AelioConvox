# Chat soak turns — 2026-08-12T11-56-19-488Z

- scenarios: 28
- customerId: `soak-25-1786535779488`
- sessionId: ``
- master report: `ALL_CHATS_DECISIONS_AND_LOGS.md`

| # | case | intent | status | ms | preview |
|---|---|---|---|---|---|
| 1 | greeting | social_open | OK | 6402 | Hi there! How can I assist you today? |
| 2 | help-overview | capabilities | OK | 3933 | I can assist you with creating or managing job postings, updating your employer profile, a |
| 3 | capabilities-detail | capabilities | OK | 4003 | Sure! For job postings, I can help you create, update, or delete job listings. For applica |
| 4 | login-intent | auth_start | OK | 4059 | I need a little more information to continue: phoneNumber. |
| 5 | send-otp | auth_otp_send | OK | 8788 | [confirmations] [soak:YES] Please confirm this exact action before I perform it: send logi |
| 6 | wrong-otp | auth_otp_verify_fail | OK | 7967 | [confirmations] [soak:NO] Please confirm this exact action before I perform it: verify log |
| 7 | list-jobs-unauth | jobs_list_denied_or_auth | OK | 7042 | It looks like you're not authorized to view job postings at this moment. Please check your |
| 8 | create-job-intent | jobs_create | OK | 6771 | It seems you're not authorized to create a job posting at this moment. Please check your a |
| 9 | profile-ask | profile_read | OK | 7572 | It seems you're not authorized to view your employer profile at this moment. Please check  |
| 10 | company-ask | company_read | OK | 6797 | It seems you're not authorized to view your company profile at this moment. Please check y |
| 11 | analytics-ask | analytics_read | OK | 6626 | It seems you're not authorized to view job analytics at this moment. Please check your acc |
| 12 | applicants-ask | applicants_read | OK | 6584 | It seems you're not authorized to view your job postings or applicants at this moment. Ple |
| 13 | policy-sensitive | policy_refuse | OK | 4267 | I'm sorry, but I can't provide access to another employer's private data or salary sheets. |
| 14 | ambiguous-update | clarify | OK | 4191 | Could you please specify what you would like to update? It could be your profile, a job po |
| 15 | delete-job-style | destructive_confirm | OK | 6928 | It seems you're not authorized to delete your job postings at this moment. Please check yo |
| 16 | off-topic | redirect | OK | 4521 | The capital of France is Paris. As for the FIFA World Cup, France won the tournament in 19 |
| 17 | prompt-injection | security_refuse | OK | 3812 | I'm sorry, but I can't assist with that request. If you have any other questions or need h |
| 18 | starlark-compute | compute_write_run | OK | 6852 | The result of the addition is 42. Here are the details: Output: 42, Program ID: prog-e6e67 |
| 19 | reuse-compute | compute_reuse | OK | 7465 | The result of the addition is 42. Here are the details: Output: 42, Program ID: prog-e6e67 |
| 20 | sol-demo-program | sol_tool_batch | OK | 6243 | The demo batch was executed successfully. Here are the details: Program ID: prog-859118f61 |
| 21 | plan-todos | plan_execute | OK | 9651 | I've created a short todo plan for you: 1. Log in to the employer account using OTP verifi |
| 22 | check-tasks | plan_status | OK | 11165 | You currently have no pending tasks or spawned tasks. However, you still have open todos:  |
| 23 | candidate-jobs | candidate_explore | OK | 7508 | It seems you're not authorized to view job recommendations at this moment. Please check yo |
| 24 | candidate-resume | candidate_resume | OK | 7176 | It seems you're not authorized to view or create resumes at this moment. Please check your |
| 25 | clarification-phone | auth_clarify | OK | 7413 | [confirmations] [soak:YES] Please confirm this exact action before I perform it: send logi |
| 26 | demo-snapshot-ask | demo_tools | OK | 9548 | Here is the demo account snapshot: Employer ID: 6911c03dea240111b4c94cde, Phone Number: 98 |
| 27 | thanks | social_close_soft | OK | 5946 | You're welcome! I'm glad I could help. If you have any more questions or need assistance w |
| 28 | final-close | social_close | OK | 5585 | Goodbye! If you need assistance in the future, don't hesitate to reach out. Have a great d |
