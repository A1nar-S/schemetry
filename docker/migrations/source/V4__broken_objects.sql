-- Deliberately uncompilable objects for the invalid-object / compile-error tests;
-- identical in source and target. Flyway treats compilation errors as warnings, so the
-- migration still succeeds. Schema compare only looks at tables, so these don't
-- show up as discrepancies.

CREATE OR REPLACE FORCE VIEW BROKEN_VIEW AS
SELECT ID FROM NO_SUCH_TABLE;

CREATE OR REPLACE PROCEDURE BROKEN_PROC AS
BEGIN
    NO_SUCH_PROCEDURE(1);
END;
/
