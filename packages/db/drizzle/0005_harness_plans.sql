CREATE TABLE IF NOT EXISTS `plans` (
	`id` text PRIMARY KEY NOT NULL,
	`session_id` text NOT NULL,
	`customer_id` text NOT NULL,
	`turn_id` text NOT NULL,
	`status` text DEFAULT 'pending' NOT NULL,
	`matched_flow_id` text,
	`user_message` text NOT NULL,
	`intent` text,
	`intent_category` text,
	`token_spend` integer DEFAULT 0 NOT NULL,
	`replan_count` integer DEFAULT 0 NOT NULL,
	`abort_reason` text,
	`created_at` integer NOT NULL,
	`updated_at` integer NOT NULL,
	FOREIGN KEY (`session_id`) REFERENCES `sessions`(`id`) ON UPDATE no action ON DELETE no action,
	FOREIGN KEY (`customer_id`) REFERENCES `customers`(`id`) ON UPDATE no action ON DELETE no action
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS `idx_plans_session` ON `plans` (`session_id`);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS `idx_plans_status` ON `plans` (`status`,`created_at`);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS `plan_steps` (
	`id` text PRIMARY KEY NOT NULL,
	`plan_id` text NOT NULL,
	`step_order` integer NOT NULL,
	`tool_name` text NOT NULL,
	`depends_on` text DEFAULT '[]' NOT NULL,
	`input_template` text NOT NULL,
	`resolved_input` text,
	`output` text,
	`status` text DEFAULT 'pending' NOT NULL,
	`idempotency_key` text NOT NULL,
	`attempt_count` integer DEFAULT 0 NOT NULL,
	`error_message` text,
	`started_at` integer,
	`finished_at` integer,
	FOREIGN KEY (`plan_id`) REFERENCES `plans`(`id`) ON UPDATE no action ON DELETE cascade
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS `idx_plan_steps_plan` ON `plan_steps` (`plan_id`,`step_order`);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS `plan_steps_idempotency_unique` ON `plan_steps` (`plan_id`,`idempotency_key`);
