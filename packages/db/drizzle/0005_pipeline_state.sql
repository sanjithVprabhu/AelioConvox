CREATE TABLE `customer_pipeline_state` (
	`customer_id` text PRIMARY KEY NOT NULL,
	`global_stage` text NOT NULL,
	`entered_at` integer NOT NULL,
	`entry_method` text DEFAULT 'system_triggered' NOT NULL,
	`metadata` text,
	FOREIGN KEY (`customer_id`) REFERENCES `customers`(`id`) ON UPDATE no action ON DELETE no action
);
--> statement-breakpoint
CREATE INDEX `idx_pipeline_state_stage` ON `customer_pipeline_state` (`global_stage`);--> statement-breakpoint
CREATE TABLE `customer_feature_state` (
	`id` text PRIMARY KEY NOT NULL,
	`customer_id` text NOT NULL,
	`feature_id` text NOT NULL,
	`stage` text NOT NULL,
	`entered_at` integer NOT NULL,
	`entry_method` text DEFAULT 'system_triggered' NOT NULL,
	FOREIGN KEY (`customer_id`) REFERENCES `customers`(`id`) ON UPDATE no action ON DELETE no action
);
--> statement-breakpoint
CREATE UNIQUE INDEX `idx_feature_state_customer_feature` ON `customer_feature_state` (`customer_id`,`feature_id`);--> statement-breakpoint
CREATE TABLE `customer_attributes` (
	`id` text PRIMARY KEY NOT NULL,
	`customer_id` text NOT NULL,
	`attribute_id` text NOT NULL,
	`value` text NOT NULL,
	`source` text DEFAULT 'explicit_ask' NOT NULL,
	`verified` integer DEFAULT false,
	`collected_at` integer NOT NULL,
	FOREIGN KEY (`customer_id`) REFERENCES `customers`(`id`) ON UPDATE no action ON DELETE no action
);
--> statement-breakpoint
CREATE UNIQUE INDEX `idx_customer_attribute` ON `customer_attributes` (`customer_id`,`attribute_id`);
