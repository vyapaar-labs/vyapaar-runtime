use std::sync::Arc;

use chrono::{Duration, TimeZone, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use vyapaar_agent_chat_tools::ports::market::{
    InstrumentSearchResult, MacroSeries, MarketBar, MarketQuote, MarketReader,
};
use vyapaar_agent_chat_tools::ports::portfolio::{DateRange, OrderFilter, PortfolioReader};
use vyapaar_agent_chat_tools::ports::snapshot::{SnapshotNeeds, SnapshotSource};
use vyapaar_core::{
    AccountId, AlgoId, ClientOrderId, Currency, Exchange, InstrumentId, Money, OrderSide,
    OrderStatus, OrderType, PositionSide, Price, PriceBand, Quantity, TimeInForce,
};
use vyapaar_ledger::TaxLot;
use vyapaar_ledger::repository::{
    AccountRecord, AccountRepository, InMemoryAccountRepository, InMemoryLedgerRepository,
    InMemoryOrderRepository, InMemoryPositionRepository, OrderRecord, OrderRepository,
    PositionRepository,
};
use vyapaar_runtime_api::chat_adapters::{
    LedgerPortfolioReader, RuntimeMarketReader, RuntimeSnapshotSource,
};

#[tokio::test]
async fn test_ledger_portfolio_reader() {
    let mut accounts = InMemoryAccountRepository::new();
    let mut positions = InMemoryPositionRepository::new();
    let mut orders = InMemoryOrderRepository::new();
    let ledger = InMemoryLedgerRepository::new(Currency::Inr);

    let account_id = AccountId::new("ACCT-001");
    let algo_id = AlgoId::new("ALGO-MOMENTUM");
    let now = Utc::now();

    // 1. Setup account
    let acct_rec = AccountRecord {
        account_id: account_id.clone(),
        algo_id: Some(algo_id.clone()),
        cash: Money::new(dec!(250000.0), Currency::Inr),
        status: "ACTIVE".to_string(),
        updated_at: now,
    };
    accounts.save_account(&acct_rec).unwrap();

    // 2. Setup positions and tax lots
    let inst_rel = InstrumentId::new(Exchange::Nse, "RELIANCE");
    let inst_tcs = InstrumentId::new(Exchange::Nse, "TCS");

    let lot = TaxLot {
        lot_id: "LOT-1".to_string(),
        account_id: account_id.clone(),
        instrument: inst_rel.clone(),
        side: PositionSide::Long,
        entry_price: Price::new(dec!(2400.0)),
        quantity: Quantity::new(dec!(50)),
        remaining_quantity: Quantity::new(dec!(50)),
        entry_date: now.date_naive(),
        created_at: now,
    };
    positions.save_lot(&account_id, &lot).unwrap();

    // Verify position query
    let pos_list = positions.get_positions(&account_id).unwrap();
    assert_eq!(pos_list.len(), 1);

    // 3. Setup orders
    let ord1 = OrderRecord {
        client_order_id: ClientOrderId::new("ORD-1"),
        broker_order_id: None,
        algo_id: Some(algo_id.clone()),
        account_id: account_id.clone(),
        instrument: inst_rel.clone(),
        side: OrderSide::Buy,
        order_type: OrderType::Limit(Price::new(dec!(2400.0))),
        time_in_force: TimeInForce::Day,
        price: Some(Price::new(dec!(2400.0))),
        quantity: Quantity::new(dec!(50)),
        filled_quantity: Quantity::new(dec!(50)),
        avg_fill_price: Some(Price::new(dec!(2400.0))),
        status: OrderStatus::Filled,
        created_at: now,
        updated_at: now,
    };
    let ord2 = OrderRecord {
        client_order_id: ClientOrderId::new("ORD-2"),
        broker_order_id: None,
        algo_id: Some(algo_id.clone()),
        account_id: account_id.clone(),
        instrument: inst_tcs.clone(),
        side: OrderSide::Buy,
        order_type: OrderType::Limit(Price::new(dec!(3500.0))),
        time_in_force: TimeInForce::Day,
        price: Some(Price::new(dec!(3500.0))),
        quantity: Quantity::new(dec!(10)),
        filled_quantity: Quantity::from_u64(0),
        avg_fill_price: None,
        status: OrderStatus::New,
        created_at: now + Duration::seconds(1),
        updated_at: now + Duration::seconds(1),
    };
    orders.save_order(&ord1).unwrap();
    orders.save_order(&ord2).unwrap();

    let reader = LedgerPortfolioReader::new(
        Arc::new(accounts),
        Arc::new(positions),
        Arc::new(orders),
        Arc::new(ledger),
    );

    // Test accounts
    let accts = reader.accounts().await.unwrap();
    assert_eq!(accts.len(), 1);
    assert_eq!(accts[0].account_id, account_id);
    assert_eq!(accts[0].cash.amount, dec!(250000.0));

    // Test positions
    let pos = reader.positions(&account_id).await.unwrap();
    assert_eq!(pos.len(), 1);
    assert_eq!(pos[0].instrument, inst_rel);

    // Test tax_lots
    let lots = reader.tax_lots(&account_id, &inst_rel).await.unwrap();
    assert_eq!(lots.len(), 1);
    assert_eq!(lots[0].lot_id, "LOT-1");

    let empty_lots = reader.tax_lots(&account_id, &inst_tcs).await.unwrap();
    assert!(empty_lots.is_empty());

    // Test cash
    let cash = reader.cash(&account_id).await.unwrap();
    assert_eq!(cash.len(), 1);
    assert_eq!(cash[0].amount, dec!(250000.0));

    // Test orders (active only)
    let active_orders = reader
        .orders(
            &account_id,
            OrderFilter {
                active_only: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(active_orders.len(), 1);
    assert_eq!(active_orders[0].client_order_id.as_str(), "ORD-2");

    // Test realized PnL
    let pnl = reader
        .realized_pnl(&account_id, DateRange::default())
        .await
        .unwrap();
    assert_eq!(pnl.amount, dec!(0.0));
}

#[tokio::test]
async fn test_runtime_market_reader() {
    let reader = RuntimeMarketReader::new();
    let inst = InstrumentId::new(Exchange::Nse, "INFY");
    let now = Utc.with_ymd_and_hms(2026, 11, 6, 9, 15, 0).unwrap();

    // 1. Quote
    let quote = MarketQuote {
        instrument: inst.clone(),
        last_price: Price::new(dec!(1850.50)),
        bid: Some(Price::new(dec!(1850.00))),
        ask: Some(Price::new(dec!(1851.00))),
        volume: Quantity::new(dec!(100000)),
        as_of: now,
    };
    reader.set_quote(quote.clone());
    let fetched = reader.quote(&inst).await.unwrap();
    assert_eq!(fetched.last_price.0, dec!(1850.50));

    // 2. History & downsampling test (>500 bars capped at 500)
    let mut bars = Vec::new();
    for i in 0..600 {
        bars.push(MarketBar {
            timestamp: now + Duration::minutes(i),
            open: Price::new(dec!(1800.0) + Decimal::from(i)),
            high: Price::new(dec!(1810.0) + Decimal::from(i)),
            low: Price::new(dec!(1790.0) + Decimal::from(i)),
            close: Price::new(dec!(1805.0) + Decimal::from(i)),
            volume: Quantity::new(dec!(1000)),
        });
    }
    reader.set_history(inst.clone(), bars);

    let hist = reader
        .history(&inst, DateRange::default(), None)
        .await
        .unwrap();
    assert_eq!(hist.bars.len(), 500);
    assert!(hist.downsampled);

    // Filter with date range
    let range = DateRange::new(
        Some(now + Duration::minutes(10)),
        Some(now + Duration::minutes(20)),
    );
    let hist_ranged = reader.history(&inst, range, None).await.unwrap();
    assert_eq!(hist_ranged.bars.len(), 10);
    assert!(!hist_ranged.downsampled);

    // 3. Indicators
    reader.set_indicator(inst.clone(), "rsi_14", dec!(64.2));
    reader.set_indicator(inst.clone(), "ema_20", dec!(1842.10));
    let ind = reader
        .indicators(&inst, &["rsi_14".to_string(), "ema_20".to_string()])
        .await
        .unwrap();
    assert_eq!(ind.values.get("rsi_14"), Some(&dec!(64.2)));
    assert_eq!(ind.values.get("ema_20"), Some(&dec!(1842.10)));

    // 4. Instrument search
    reader.add_instrument(InstrumentSearchResult {
        instrument: inst.clone(),
        name: "Infosys Limited".to_string(),
        exchange: "NSE".to_string(),
        lot_size: Some(1),
    });
    let search_hits = reader.search_instruments("infosys").await.unwrap();
    assert_eq!(search_hits.len(), 1);
    assert_eq!(search_hits[0].instrument, inst);

    // 5. Macro series
    reader.set_macro_series(MacroSeries {
        series: "cpi".to_string(),
        points: vec![(now, dec!(5.4))],
    });
    let macro_res = reader.macro_series("cpi").await.unwrap();
    assert_eq!(macro_res.points[0].1, dec!(5.4));

    // 6. Price band
    reader.set_price_band(
        inst.clone(),
        PriceBand::new(
            Price::new(dec!(1665.0)),
            Price::new(dec!(2035.0)),
            Price::new(dec!(1850.0)),
        ),
    );
    let band = reader.price_band(&inst).await.unwrap();
    assert_eq!(band.upper.0, dec!(2035.0));
}

#[tokio::test]
async fn test_runtime_snapshot_source() {
    let source = RuntimeSnapshotSource::new();
    source.set_universe(vec!["NIFTY50".to_string(), "BANKNIFTY".to_string()]);
    source.set_regime("TRENDING_BULL");
    source.set_risk_metric("circuit_filter_hit", serde_json::json!(false));

    let snap = source.snapshot(&SnapshotNeeds::default()).await.unwrap();
    assert_eq!(
        snap.universe,
        Some(vec!["NIFTY50".to_string(), "BANKNIFTY".to_string()])
    );
    assert_eq!(snap.regime, Some("TRENDING_BULL".to_string()));
    assert_eq!(
        snap.risk_metrics.get("circuit_filter_hit"),
        Some(&serde_json::json!(false))
    );
}
