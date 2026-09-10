-- Postgres counterpart to docker/migrations/source/V1__init.sql. Reference schema used
-- by the Postgres integration tests. The "pg-target" migration
-- (docker/migrations/pg-target/V1__init.sql) intentionally diverges from this one:
--   - it has no audit_log table
--   - employees has no email column
--   - employees.first_name is shorter (varchar(30) instead of varchar(50))
-- so that schema-compare has real discrepancies to detect and fix.

CREATE TABLE departments (
    dept_id   integer      NOT NULL,
    dept_name varchar(50)  NOT NULL,
    CONSTRAINT pk_departments PRIMARY KEY (dept_id)
);

COMMENT ON TABLE departments IS 'Company departments';
COMMENT ON COLUMN departments.dept_name IS 'Department display name';

CREATE TABLE employees (
    emp_id     integer        NOT NULL,
    first_name varchar(50)    NOT NULL,
    last_name  varchar(50)    NOT NULL,
    email      varchar(100),
    salary     numeric(10,2)  DEFAULT 0,
    dept_id    integer,
    CONSTRAINT pk_employees PRIMARY KEY (emp_id),
    CONSTRAINT fk_emp_dept FOREIGN KEY (dept_id) REFERENCES departments (dept_id)
);

COMMENT ON COLUMN employees.email IS 'Work email address';

CREATE TABLE audit_log (
    log_id     integer       NOT NULL,
    action     varchar(200)  NOT NULL,
    created_at timestamp     DEFAULT now(),
    CONSTRAINT pk_audit_log PRIMARY KEY (log_id)
);

INSERT INTO departments (dept_id, dept_name) VALUES (10, 'Engineering');
INSERT INTO departments (dept_id, dept_name) VALUES (20, 'Sales');

INSERT INTO employees (emp_id, first_name, last_name, email, salary, dept_id)
VALUES (100, 'Ada', 'Lovelace', 'ada@schemetry.test', 9500.50, 10);
INSERT INTO employees (emp_id, first_name, last_name, email, salary, dept_id)
VALUES (101, 'Grace', 'Hopper', 'grace@schemetry.test', 9800.00, 10);
