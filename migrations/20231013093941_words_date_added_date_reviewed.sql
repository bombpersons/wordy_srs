-- Add migration script here

-- First add columns to the table
ALTER TABLE words 
ADD COLUMN date_first_reviewed TEXT DEFAULT NULL;

ALTER TABLE words
ADD COLUMN date_added TEXT DEFAULT NULL;

-- Set any words date_first_reviewed value to now if
-- they have been reviewed already.
UPDATE words
SET date_first_reviewed = strftime("%Y-%m-%dT%H:%M:%SZ", datetime("now"))
WHERE date_first_reviewed IS NULL
	  AND reviewed = TRUE;
	
-- Just use now date for when the word was added.
UPDATE words
SET date_added = strftime("%Y-%m-%dT%H:%M:%SZ", datetime("now"));

-- Create a new table with a NOT NULL constraint on date_added.
CREATE TEMP TABLE words_temp (
    id INTEGER PRIMARY KEY,
    text TEXT NOT NULL,
    count INTEGER DEFAULT 1,
    frequency INTEGER,

    reviewed INT DEFAULT 0,
    next_review_at TEXT,

	date_added TEXT NOT NULL,
	date_first_reviewed TEXT,

    review_duration INTEGER DEFAULT 0,
    e_factor REAL DEFAULT 0,
    repitition INTEGER DEFAULT 0,

    UNIQUE(text)
);

-- Now copy over the old rows to the new one.
INSERT INTO words_temp (id, text, count, frequency, reviewed, next_review_at, date_added, date_first_reviewed, review_duration, e_factor, repitition)
	SELECT id, text, count, frequency, reviewed, next_review_at, date_added, date_first_reviewed, review_duration, e_factor, repitition
	FROM words;

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
DROP TABLE words;
DROP TABLE word_sentence;

-- Make new tables and re-insert the data.
CREATE TABLE words (
    id INTEGER PRIMARY KEY,
    text TEXT NOT NULL,
    count INTEGER DEFAULT 1,
    frequency INTEGER,

    reviewed INT DEFAULT 0,
    next_review_at TEXT,

	date_added TEXT NOT NULL,
	date_first_reviewed TEXT,

    review_duration INTEGER DEFAULT 0,
    e_factor REAL DEFAULT 0,
    repitition INTEGER DEFAULT 0,

    UNIQUE(text)
);
INSERT INTO words (id, text, count, frequency, reviewed, next_review_at, date_added, date_first_reviewed, review_duration, e_factor, repitition)
	SELECT id, text, count, frequency, reviewed, next_review_at, date_added, date_first_reviewed, review_duration, e_factor, repitition
	FROM words_temp;

CREATE TABLE word_sentence (
    word_id INTEGER NOT NULL REFERENCES words(id) ON DELETE CASCADE,
    sentence_id INTEGER NOT NULL REFERENCES sentences(id) ON DELETE CASCADE,
    PRIMARY KEY (word_id, sentence_id)
);
INSERT INTO word_sentence (word_id, sentence_id)
	SELECT word_id, sentence_id
	FROM word_sentence_temp;

-- Finally drop the temp tables.
DROP TABLE words_temp;
DROP TABLE word_sentence_temp;