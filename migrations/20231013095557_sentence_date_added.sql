-- Add migration script here
ALTER TABLE sentences 
ADD COLUMN date_added TEXT DEFAULT NULL;

UPDATE sentences
SET date_added = strftime("%Y-%m-%dT%H:%M:%SZ", datetime("now"));

-- Create a temp table to copy sentences over.
CREATE TEMP TABLE sentences_temp (
    id INTEGER PRIMARY KEY,
    text TEXT NOT NULL,

    date_added TEXT NOT NULL,

    UNIQUE(text)
);

-- Copy data over.
INSERT INTO sentences_temp (id, text, date_added)
    SELECT id, text, date_added
    FROM sentences;

-- Create a temp table for the word_sentence relationship so that we can save the relationships.
CREATE TEMP TABLE word_sentence_temp (
    word_id INTEGER NOT NULL,
    sentence_id INTEGER NOT NULL,
    PRIMARY KEY (word_id, sentence_id)
);

-- Copy over data for this table
INSERT INTO word_sentence_temp (word_id, sentence_id)
	SELECT word_id, sentence_id
	FROM word_sentence;

-- Now drop the old tables.
DROP TABLE sentences;
DROP TABLE word_sentence;

-- Create new tables.
CREATE TABLE sentences (
    id INTEGER PRIMARY KEY,
    text TEXT NOT NULL,

    date_added TEXT NOT NULL,

    UNIQUE(text)
);
INSERT INTO sentences (id, text, date_added)
    SELECT id, text, date_added
    FROM sentences_temp;

CREATE TABLE word_sentence (
    word_id INTEGER NOT NULL REFERENCES words(id) ON DELETE CASCADE,
    sentence_id INTEGER NOT NULL REFERENCES sentences(id) ON DELETE CASCADE,
    PRIMARY KEY (word_id, sentence_id)
);
INSERT INTO word_sentence (word_id, sentence_id)
	SELECT word_id, sentence_id
	FROM word_sentence_temp;

-- Drop the temp tables.
DROP TABLE sentences_temp;
DROP TABLE word_sentence_temp;
