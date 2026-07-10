CREATE TABLE IF NOT EXISTS `tool_embeddings` (
	`tool_name` text PRIMARY KEY NOT NULL,
	`descriptor` text NOT NULL,
	`descriptor_hash` text NOT NULL,
	`embedding` text NOT NULL,
	`updated_at` integer NOT NULL
);
