-- Postgres counterpart to source/V2__lob_documents.sql; identical in pg-source and
-- pg-target. Rows: 1 = short text + PNG bytes, 2 = NULLs, 3 = 3,000,000 chars/bytes
-- (block i of 1..300 is 10,000 copies of the digit i % 10).

CREATE TABLE lob_documents (
    doc_id  integer       NOT NULL,
    title   varchar(100)  NOT NULL,
    body    text,
    payload bytea,
    CONSTRAINT pk_lob_documents PRIMARY KEY (doc_id)
);

INSERT INTO lob_documents (doc_id, title, body, payload)
VALUES (1, 'short', 'Price: 5 € — ✓ done', '\x89504e470d0a1a0a00000000'::bytea);

INSERT INTO lob_documents (doc_id, title, body, payload)
VALUES (2, 'nulls', NULL, NULL);

INSERT INTO lob_documents (doc_id, title, body, payload)
SELECT 3, 'large', string_agg(repeat((i % 10)::text, 10000), '' ORDER BY i),
       convert_to(string_agg(repeat((i % 10)::text, 10000), '' ORDER BY i), 'UTF8')
FROM generate_series(1, 300) AS i;
