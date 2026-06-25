CREATE TABLE `magic_links` (
	`id` text PRIMARY KEY NOT NULL,
	`token_hash` text NOT NULL,
	`email` text NOT NULL,
	`customer_id` text,
	`external_id` text,
	`expires_at` integer NOT NULL,
	`consumed_at` integer,
	`created_at` integer NOT NULL,
	FOREIGN KEY (`customer_id`) REFERENCES `customers`(`id`) ON UPDATE no action ON DELETE no action
);
