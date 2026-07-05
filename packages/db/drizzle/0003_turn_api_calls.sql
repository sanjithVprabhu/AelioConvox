CREATE TABLE IF NOT EXISTS `turn_api_calls` (
	`id` text PRIMARY KEY NOT NULL,
	`turn_id` text NOT NULL,
	`session_id` text NOT NULL,
	`customer_id` text NOT NULL,
	`sequence` integer NOT NULL,
	`call_type` text NOT NULL,
	`purpose` text NOT NULL,
	`model` text,
	`iteration` integer,
	`prompt_summary` text NOT NULL,
	`input_preview` text,
	`message_count` integer,
	`tool_count` integer,
	`tool_names` text,
	`stop_reason` text,
	`duration_ms` integer,
	`created_at` integer NOT NULL,
	FOREIGN KEY (`session_id`) REFERENCES `sessions`(`id`) ON UPDATE no action ON DELETE no action,
	FOREIGN KEY (`customer_id`) REFERENCES `customers`(`id`) ON UPDATE no action ON DELETE no action
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS `idx_turn_api_calls_turn` ON `turn_api_calls` (`turn_id`,`sequence`);--> statement-breakpoint
CREATE INDEX IF NOT EXISTS `idx_turn_api_calls_session` ON `turn_api_calls` (`session_id`,`created_at`);
