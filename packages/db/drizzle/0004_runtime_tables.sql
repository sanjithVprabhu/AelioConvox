CREATE TABLE IF NOT EXISTS `reflections` (
	`id` text PRIMARY KEY NOT NULL,
	`session_id` text NOT NULL,
	`customer_id` text NOT NULL,
	`outcome` text NOT NULL,
	`score` real,
	`summary` text,
	`issues` text,
	`created_at` integer NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS `idx_reflections_session` ON `reflections` (`session_id`);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS `idx_reflections_customer` ON `reflections` (`customer_id`,`created_at`);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS `proactive_messages` (
	`id` text PRIMARY KEY NOT NULL,
	`customer_id` text NOT NULL,
	`channel` text NOT NULL,
	`to_address` text NOT NULL,
	`content` text NOT NULL,
	`dedup_key` text,
	`status` text NOT NULL,
	`reason` text,
	`created_at` integer NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS `idx_proactive_customer` ON `proactive_messages` (`customer_id`,`created_at`);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS `inbound_dedup` (
	`message_id` text PRIMARY KEY NOT NULL,
	`created_at` integer NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS `response_cache` (
	`id` text PRIMARY KEY NOT NULL,
	`customer_id` text NOT NULL,
	`query` text NOT NULL,
	`embedding` text,
	`reply` text NOT NULL,
	`hits` integer DEFAULT 0,
	`created_at` integer NOT NULL,
	`expires_at` integer NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS `idx_respcache_customer` ON `response_cache` (`customer_id`,`expires_at`);
