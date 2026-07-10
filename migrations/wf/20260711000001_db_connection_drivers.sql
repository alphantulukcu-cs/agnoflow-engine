-- Autoexec SQL sürücü genişletmesi: sqlite + wire-protokol alias'ları.
-- Engine tarafında mariadb/tidb→mysql, cockroachdb/redshift/timescaledb→postgres,
-- sqlserver→mssql olarak çözülür (crates/wfe/src/db/mod.rs DbDriver::parse).
ALTER TABLE wf.db_connection DROP CONSTRAINT IF EXISTS db_connection_driver_check;
ALTER TABLE wf.db_connection ADD CONSTRAINT db_connection_driver_check
  CHECK (driver IN (
    'postgres', 'mysql', 'mariadb', 'mssql', 'sqlite',
    'cockroachdb', 'redshift', 'timescaledb', 'tidb'
  ));
