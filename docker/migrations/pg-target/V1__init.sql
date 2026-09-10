-- Deliberately divergent counterpart to docker/migrations/pg-source/V1__init.sql - see
-- that file for the full list of differences this is designed to exercise.

CREATE TABLE departments (
    dept_id   integer      NOT NULL,
    dept_name varchar(50)  NOT NULL,
    CONSTRAINT pk_departments PRIMARY KEY (dept_id)
);

COMMENT ON TABLE departments IS 'Company departments';
COMMENT ON COLUMN departments.dept_name IS 'Department display name';

CREATE TABLE employees (
    emp_id     integer        NOT NULL,
    first_name varchar(30)    NOT NULL,
    last_name  varchar(50)    NOT NULL,
    salary     numeric(10,2)  DEFAULT 0,
    dept_id    integer,
    CONSTRAINT pk_employees PRIMARY KEY (emp_id),
    CONSTRAINT fk_emp_dept FOREIGN KEY (dept_id) REFERENCES departments (dept_id)
);

INSERT INTO departments (dept_id, dept_name) VALUES (10, 'Engineering');
INSERT INTO departments (dept_id, dept_name) VALUES (20, 'Sales');

INSERT INTO employees (emp_id, first_name, last_name, salary, dept_id)
VALUES (100, 'Ada', 'Lovelace', 9500.50, 10);
INSERT INTO employees (emp_id, first_name, last_name, salary, dept_id)
VALUES (101, 'Grace', 'Hopper', 9800.00, 10);
