CREATE TABLE departments (
    id               SERIAL PRIMARY KEY,
    name             VARCHAR NOT NULL,
    head_employee_id INTEGER
);

CREATE TABLE employees (
    id            SERIAL PRIMARY KEY,
    name          VARCHAR NOT NULL,
    department_id INTEGER NOT NULL
);

ALTER TABLE departments
    ADD CONSTRAINT departments_head_employee_id_fkey
    FOREIGN KEY (head_employee_id) REFERENCES employees(id);

ALTER TABLE employees
    ADD CONSTRAINT employees_department_id_fkey
    FOREIGN KEY (department_id) REFERENCES departments(id);
