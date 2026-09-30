-- LOB fixture for the query-view tests; identical in source and target.
-- Rows: 1 = short text + PNG bytes, 2 = NULLs, 3 = 3,000,000 chars/bytes.

CREATE TABLE LOB_DOCUMENTS (
    DOC_ID  NUMBER(6)     NOT NULL,
    TITLE   VARCHAR2(100) NOT NULL,
    BODY    CLOB,
    PAYLOAD BLOB,
    CONSTRAINT PK_LOB_DOCUMENTS PRIMARY KEY (DOC_ID)
);

INSERT INTO LOB_DOCUMENTS (DOC_ID, TITLE, BODY, PAYLOAD)
VALUES (1, 'short', TO_CLOB('Price: 5 € — ✓ done'), HEXTORAW('89504E470D0A1A0A00000000'));

INSERT INTO LOB_DOCUMENTS (DOC_ID, TITLE, BODY, PAYLOAD)
VALUES (2, 'nulls', NULL, NULL);

-- Block i (1..300) is 10,000 copies of the digit MOD(i, 10).
DECLARE
    big_text CLOB;
    big_bin  BLOB;
    block    VARCHAR2(10000);
BEGIN
    DBMS_LOB.CREATETEMPORARY(big_text, TRUE);
    DBMS_LOB.CREATETEMPORARY(big_bin, TRUE);
    FOR i IN 1 .. 300 LOOP
        block := RPAD(TO_CHAR(MOD(i, 10)), 10000, TO_CHAR(MOD(i, 10)));
        DBMS_LOB.WRITEAPPEND(big_text, 10000, block);
        DBMS_LOB.WRITEAPPEND(big_bin, 10000, UTL_RAW.CAST_TO_RAW(block));
    END LOOP;
    INSERT INTO LOB_DOCUMENTS (DOC_ID, TITLE, BODY, PAYLOAD)
    VALUES (3, 'large', big_text, big_bin);
    DBMS_LOB.FREETEMPORARY(big_text);
    DBMS_LOB.FREETEMPORARY(big_bin);
END;
/

COMMIT;
