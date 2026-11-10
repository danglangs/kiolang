# Import orders into SQLite

`main` receives `input: Dir`, granted for reading, and `shop: Db`, a SQLite database that keeps its contents between runs (empty on the first run; create your schema if it is missing).

`input/orders.csv` has a header line, then rows `order_id,customer,product,qty,unit_cents`. Import the rows into the database. An `order_id` already imported, in this run or an earlier one, is skipped (exports overlap). Rows that do not have five fields or whose `order_id`, `qty` or `unit_cents` is not an integer are rejected. Blank lines are ignored.

Print `imported N skipped S rejected R` for this run, then the top 3 products over all imported orders by revenue (`qty * unit_cents`), as `PRODUCT REVENUE_CENTS ORDERS`, highest revenue first, equal revenue by product name (byte order).

Grants: `input: Dir`, `shop: Db`
