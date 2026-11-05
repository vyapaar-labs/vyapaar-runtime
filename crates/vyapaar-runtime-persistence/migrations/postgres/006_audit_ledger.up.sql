-- Create audit ledger table for SEBI compliance
CREATE TABLE IF NOT EXISTS sebi_audit_ledger (
    id SERIAL PRIMARY KEY,
    transaction_id UUID NOT NULL,
    timestamp TIMESTAMPTZ NOT NULL,
    description TEXT NOT NULL,
    debit_account TEXT NOT NULL,
    credit_account TEXT NOT NULL,
    amount NUMERIC(18, 8) NOT NULL,
    currency TEXT NOT NULL DEFAULT 'INR',
    client_id TEXT,
    algo_id TEXT,
    instrument TEXT,
    order_id TEXT,
    fill_id TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_audit_ledger_transaction_id ON sebi_audit_ledger(transaction_id);
CREATE INDEX IF NOT EXISTS idx_audit_ledger_timestamp ON sebi_audit_ledger(timestamp);
CREATE INDEX IF NOT EXISTS idx_audit_ledger_client_id ON sebi_audit_ledger(client_id);
CREATE INDEX IF NOT EXISTS idx_audit_ledger_instrument ON sebi_audit_ledger(instrument);
