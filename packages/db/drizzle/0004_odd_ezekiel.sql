CREATE TABLE `harness_ledger` (
	`id` text PRIMARY KEY NOT NULL,
	`session_id` text NOT NULL,
	`turn_id` text NOT NULL,
	`instruction_id` text NOT NULL,
	`args_hash` text NOT NULL,
	`status` text NOT NULL,
	`result` text,
	`created_at` integer NOT NULL
);
--> statement-breakpoint
CREATE UNIQUE INDEX `idx_ledger_idem` ON `harness_ledger` (`session_id`,`instruction_id`,`args_hash`);--> statement-breakpoint
CREATE INDEX `idx_ledger_turn` ON `harness_ledger` (`turn_id`);--> statement-breakpoint
CREATE TABLE `suspended_plans` (
	`id` text PRIMARY KEY NOT NULL,
	`session_id` text NOT NULL,
	`reason` text NOT NULL,
	`payload` text NOT NULL,
	`created_at` integer NOT NULL,
	`expires_at` integer NOT NULL
);
--> statement-breakpoint
CREATE UNIQUE INDEX `idx_suspended_session` ON `suspended_plans` (`session_id`);
