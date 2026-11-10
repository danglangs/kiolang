CREATE TABLE customers (id INTEGER PRIMARY KEY, name TEXT NOT NULL, country TEXT NOT NULL);
CREATE TABLE orders (id INTEGER PRIMARY KEY, customer_id INTEGER NOT NULL REFERENCES customers(id),
                     placed TEXT NOT NULL, status TEXT NOT NULL, total_cents INTEGER NOT NULL);
INSERT INTO customers VALUES
  (1, 'acme', 'US'), (2, 'globex', 'US'), (3, 'initech', 'DE'), (4, 'umbrella', 'UK'),
  (5, 'hooli', 'US'), (6, 'stark', 'DE'), (7, 'wayne', 'FR'), (8, 'tyrell', 'UK');
INSERT INTO orders VALUES
  (1, 1, '2026-01-05', 'paid', 12000),
  (2, 1, '2026-02-11', 'paid', 3000),
  (3, 2, '2026-02-12', 'paid', 15000),
  (4, 2, '2025-12-30', 'paid', 99999),
  (5, 3, '2026-03-01', 'paid', 8000),
  (6, 3, '2026-03-02', 'refunded', 8000),
  (7, 6, '2026-04-10', 'paid', 8000),
  (8, 4, '2026-05-01', 'pending', 5000),
  (9, 4, '2026-05-02', 'paid', 2500),
  (10, 7, '2027-01-01', 'paid', 70000),
  (11, 7, '2026-06-01', 'refunded', 1000),
  (12, 5, '2026-07-07', 'paid', 100),
  (13, 8, '2026-08-08', 'paid', 2500),
  (14, 6, '2026-09-09', 'paid', 50),
  (15, 1, '2026-12-31', 'paid', 1);
