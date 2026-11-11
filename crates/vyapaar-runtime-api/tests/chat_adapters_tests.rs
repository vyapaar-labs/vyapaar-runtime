use std::sync::Arc;

use chrono::{Duration, TimeZone, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use vyapaar_agent_chat_tools::ports::error::PortErrorKind;
use vyapaar_agent_chat_tools::ports::market::{
    BlockDeal, CorporateAction, InstrumentSearchResult, MacroSeries, MarketBar, MarketQuote,
    MarketReader, OptionContract, OptionsChain,
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

    // 7. Block deals — explicitly unavailable until seeded, then filtered by instrument/date
    let err = reader.block_deals(None, None).await.unwrap_err();
    assert_eq!(err.kind, PortErrorKind::Unavailable);

    let deal_rel = BlockDeal {
        instrument: inst.clone(),
        exchange: "NSE".to_string(),
        client_name: "MORGAN STANLEY ASIA SINGAPORE PTE".to_string(),
        side: "buy".to_string(),
        category: "block".to_string(),
        quantity: Quantity::new(dec!(500000)),
        price: Price::new(dec!(1850.0)),
        trade_date: now,
    };
    let deal_tcs = BlockDeal {
        instrument: InstrumentId::new(Exchange::Nse, "TCS"),
        exchange: "BSE".to_string(),
        client_name: "GRIMALD EUROPE NC AV".to_string(),
        side: "sell".to_string(),
        category: "bulk".to_string(),
        quantity: Quantity::new(dec!(25000)),
        price: Price::new(dec!(3500.0)),
        trade_date: now + Duration::days(1),
    };
    reader.set_block_deals(vec![deal_rel.clone(), deal_tcs.clone()]);

    let all = reader.block_deals(None, None).await.unwrap();
    assert_eq!(all.len(), 2);

    let rel_only = reader.block_deals(Some(&inst), None).await.unwrap();
    assert_eq!(rel_only.len(), 1);
    assert_eq!(rel_only[0], deal_rel);

    let tcs_day = reader
        .block_deals(None, Some(now + Duration::days(1)))
        .await
        .unwrap();
    assert_eq!(tcs_day.len(), 1);
    assert_eq!(tcs_day[0], deal_tcs);

    // Same calendar day, different time-of-day: a caller asking "deals on that date" passes
    // a trading-day timestamp, not the exact intraday disclosure instant seeded above — the
    // filter must match on date, not exact `DateTime` equality.
    let tcs_day_different_hour = reader
        .block_deals(None, Some(now + Duration::days(1) + Duration::hours(5)))
        .await
        .unwrap();
    assert_eq!(tcs_day_different_hour.len(), 1);
    assert_eq!(tcs_day_different_hour[0], deal_tcs);

    // 8. Options chain — explicitly unavailable until seeded, then filtered by
    //    expiry (calendar-day) and capped by limit.
    let chain_err = reader.options_chain(&inst, None, None).await.unwrap_err();
    assert_eq!(chain_err.kind, PortErrorKind::Unavailable);

    let expiry = now + Duration::days(14);
    let chain = OptionsChain {
        underlying: inst.clone(),
        as_of: now,
        contracts: vec![
            OptionContract {
                instrument: InstrumentId::new(Exchange::Nfo, "INFY26NOV1850CE"),
                option_type: "CE".to_string(),
                strike: Price::new(dec!(1850.0)),
                expiry,
                lot_size: Quantity::new(dec!(400)),
                last_price: Some(Price::new(dec!(42.5))),
                bid: Some(Price::new(dec!(42.0))),
                ask: Some(Price::new(dec!(43.0))),
                open_interest: Some(Quantity::new(dec!(320000))),
            },
            OptionContract {
                instrument: InstrumentId::new(Exchange::Nfo, "INFY26NOV1900CE"),
                option_type: "CE".to_string(),
                strike: Price::new(dec!(1900.0)),
                expiry,
                lot_size: Quantity::new(dec!(400)),
                last_price: Some(Price::new(dec!(18.0))),
                bid: None,
                ask: None,
                open_interest: None,
            },
        ],
    };
    reader.set_options_chain(chain);

    let fetched_chain = reader.options_chain(&inst, None, None).await.unwrap();
    assert_eq!(fetched_chain.contracts.len(), 2);

    let capped_chain = reader.options_chain(&inst, None, Some(1)).await.unwrap();
    assert_eq!(capped_chain.contracts.len(), 1);

    // Same calendar-day expiry, different time-of-day: callers pass a trading-day
    // timestamp, the filter must match on date (same rule as block deals above).
    let filtered_chain = reader
        .options_chain(&inst, Some(expiry + Duration::hours(3)), None)
        .await
        .unwrap();
    assert_eq!(filtered_chain.contracts.len(), 2);

    let missing_chain = reader
        .options_chain(&InstrumentId::new(Exchange::Nse, "TCS"), None, None)
        .await
        .unwrap_err();
    assert_eq!(missing_chain.kind, PortErrorKind::NotFound);

    // 9. Corporate actions — explicitly unavailable until seeded, then filtered by
    //    instrument/action type and capped by limit.
    let actions_err = reader
        .corporate_actions(None, None, None)
        .await
        .unwrap_err();
    assert_eq!(actions_err.kind, PortErrorKind::Unavailable);

    let dividend = CorporateAction {
        instrument: inst.clone(),
        action_type: "dividend".to_string(),
        ex_date: now + Duration::days(7),
        record_date: Some(now + Duration::days(8)),
        amount: Some(Money::new(dec!(21.50), Currency::Inr)),
        ratio_from: None,
        ratio_to: None,
    };
    let split = CorporateAction {
        instrument: InstrumentId::new(Exchange::Nse, "TCS"),
        action_type: "split".to_string(),
        ex_date: now + Duration::days(30),
        record_date: None,
        amount: None,
        ratio_from: Some(Quantity::new(dec!(1))),
        ratio_to: Some(Quantity::new(dec!(2))),
    };
    reader.set_corporate_actions(vec![dividend.clone(), split.clone()]);

    let all_actions = reader.corporate_actions(None, None, None).await.unwrap();
    assert_eq!(all_actions.len(), 2);

    let dividends = reader
        .corporate_actions(Some(&inst), Some("dividend"), None)
        .await
        .unwrap();
    assert_eq!(dividends.len(), 1);
    assert_eq!(dividends[0], dividend);

    let bonus_only = reader
        .corporate_actions(None, Some("bonus"), None)
        .await
        .unwrap();
    assert!(bonus_only.is_empty());

    let capped_actions = reader.corporate_actions(None, None, Some(1)).await.unwrap();
    assert_eq!(capped_actions.len(), 1);
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
