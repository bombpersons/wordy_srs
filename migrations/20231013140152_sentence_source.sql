-- Add migration script here
ALTER TABLE sentences 
ADD COLUMN source TEXT DEFAULT "";