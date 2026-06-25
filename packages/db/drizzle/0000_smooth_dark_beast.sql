CREATE TABLE `channel_addresses` (
	`id` text PRIMARY KEY NOT NULL,
	`customer_id` text NOT NULL,
	`channel` text NOT NULL,
	`address` text NOT NULL,
	`verified_at` integer,
	`created_at` integer NOT NULL,
	FOREIGN KEY (`customer_id`) REFERENCES `customers`(`id`) ON UPDATE no action ON DELETE no action
);
--> statement-breakpoint
CREATE UNIQUE INDEX `idx_chan_addr` ON `channel_addresses` (`channel`,`address`);--> statement-breakpoint
CREATE TABLE `customers` (
	`id` text PRIMARY KEY NOT NULL,
	`external_id` text,
	`display_name` text,
	`created_at` integer NOT NULL,
	`updated_at` integer NOT NULL,
	`metadata` text
);
--> statement-breakpoint
CREATE UNIQUE INDEX `idx_customers_external` ON `customers` (`external_id`);--> statement-breakpoint
CREATE TABLE `function_calls` (
	`id` text PRIMARY KEY NOT NULL,
	`session_id` text NOT NULL,
	`customer_id` text NOT NULL,
	`function_name` text NOT NULL,
	`args` text,
	`result` text,
	`status` text NOT NULL,
	`safety_level` text NOT NULL,
	`required_confirmation` integer DEFAULT false,
	`confirmed` integer,
	`duration_ms` integer,
	`error_message` text,
	`created_at` integer NOT NULL,
	FOREIGN KEY (`session_id`) REFERENCES `sessions`(`id`) ON UPDATE no action ON DELETE no action,
	FOREIGN KEY (`customer_id`) REFERENCES `customers`(`id`) ON UPDATE no action ON DELETE no action
);
--> statement-breakpoint
CREATE INDEX `idx_fc_session` ON `function_calls` (`session_id`,`created_at`);--> statement-breakpoint
CREATE INDEX `idx_fc_customer` ON `function_calls` (`customer_id`,`created_at`);--> statement-breakpoint
CREATE TABLE `job_queue` (
	`id` text PRIMARY KEY NOT NULL,
	`queue` text NOT NULL,
	`payload` text NOT NULL,
	`status` text NOT NULL,
	`attempts` integer DEFAULT 0,
	`max_attempts` integer DEFAULT 5,
	`next_run_at` integer NOT NULL,
	`locked_by` text,
	`locked_at` integer,
	`created_at` integer NOT NULL,
	`completed_at` integer,
	`error_message` text
);
--> statement-breakpoint
CREATE INDEX `idx_jobs_pending` ON `job_queue` (`queue`,`status`,`next_run_at`);--> statement-breakpoint
CREATE TABLE `memory` (
	`id` text PRIMARY KEY NOT NULL,
	`customer_id` text NOT NULL,
	`content` text NOT NULL,
	`source_session_id` text,
	`created_at` integer NOT NULL,
	`expires_at` integer,
	`confidence` real DEFAULT 1,
	`category` text,
	FOREIGN KEY (`customer_id`) REFERENCES `customers`(`id`) ON UPDATE no action ON DELETE no action,
	FOREIGN KEY (`source_session_id`) REFERENCES `sessions`(`id`) ON UPDATE no action ON DELETE no action
);
--> statement-breakpoint
CREATE TABLE `messages` (
	`id` text PRIMARY KEY NOT NULL,
	`session_id` text NOT NULL,
	`customer_id` text NOT NULL,
	`role` text NOT NULL,
	`content` text,
	`tool_call` text,
	`tool_result` text,
	`channel` text NOT NULL,
	`created_at` integer NOT NULL,
	`tokens_in` integer,
	`tokens_out` integer,
	FOREIGN KEY (`session_id`) REFERENCES `sessions`(`id`) ON UPDATE no action ON DELETE no action,
	FOREIGN KEY (`customer_id`) REFERENCES `customers`(`id`) ON UPDATE no action ON DELETE no action
);
--> statement-breakpoint
CREATE INDEX `idx_messages_session` ON `messages` (`session_id`,`created_at`);--> statement-breakpoint
CREATE TABLE `sdk_connections` (
	`id` text PRIMARY KEY NOT NULL,
	`connection_token` text NOT NULL,
	`sdk_version` text,
	`language` text,
	`connected_at` integer NOT NULL,
	`last_heartbeat_at` integer NOT NULL,
	`functions` text NOT NULL
);
--> statement-breakpoint
CREATE TABLE `sessions` (
	`id` text PRIMARY KEY NOT NULL,
	`customer_id` text NOT NULL,
	`channel` text NOT NULL,
	`status` text NOT NULL,
	`started_at` integer NOT NULL,
	`last_activity_at` integer NOT NULL,
	`closed_at` integer,
	`summary` text,
	`metadata` text,
	FOREIGN KEY (`customer_id`) REFERENCES `customers`(`id`) ON UPDATE no action ON DELETE no action
);
--> statement-breakpoint
CREATE INDEX `idx_sessions_customer` ON `sessions` (`customer_id`,`last_activity_at`);--> statement-breakpoint
CREATE INDEX `idx_sessions_active` ON `sessions` (`status`,`last_activity_at`);