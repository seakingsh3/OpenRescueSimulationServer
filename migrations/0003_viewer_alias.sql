-- 0003_viewer_alias.sql
PRAGMA foreign_keys = ON;

ALTER TABLE viewers ADD COLUMN alias TEXT;
