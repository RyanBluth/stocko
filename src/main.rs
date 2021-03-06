#[macro_use]
extern crate serde_derive;

#[macro_use]
extern crate clap;

extern crate serde;
extern crate serde_json;

extern crate term_table;

extern crate alphavantage;

extern crate ansi_term;

use term_table::cell::{Alignment, Cell};
use term_table::row::Row;
use term_table::Table;

use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;

use alphavantage::time_series::TimeSeries;

use clap::{App, Arg, SubCommand};

use ansi_term::Colour::{Green, Red};

macro_rules! mapStockoErr {
    ($s:expr, $e:expr) => {
        $e.map_err(|e| -> StockoError { $s(e.to_string()) })
    };
}

enum StockoError {
    SaveDataError(String),
    ReadDataError(String),
    AlphaVantageError(String),
    InvalidExchange,
    InvalidShareQuantity { symbol: String, shares: u32 },
}

impl Debug for StockoError {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        match *self {
            StockoError::SaveDataError(ref e) => {
                write!(f, "Failed to save stocko_data.json. Cause: {}", e)
            }
            StockoError::ReadDataError(ref e) => {
                write!(f, "Failed to read stocko_data.json. Cause: {}", e)
            }
            StockoError::AlphaVantageError(ref e) => write!(
                f,
                "Error occured when fetching data from AlphaVantage. Cause: {}",
                e
            ),
            StockoError::InvalidExchange => write!(f, "Invalid exchange symbol"),
            StockoError::InvalidShareQuantity { ref symbol, shares } => write!(
                f,
                "You do not have {} shares of {} in your portfolio",
                shares, symbol
            ),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
enum Currency {
    CAD,
    USD,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
enum Exchange {
    TSX,
    TSXV,
    NYSE,
}

impl Default for Exchange {
    fn default() -> Self {
        return Exchange::NYSE;
    }
}

impl Exchange {
    fn from_symbol(symbol: Option<&str>) -> Result<Exchange, StockoError> {
        if let Some(symbol) = symbol {
            return match symbol.to_lowercase().as_ref() {
                "tsx" => Ok(Exchange::TSX),
                "tsxv" => Ok(Exchange::TSXV),
                "nsye" => Ok(Exchange::NYSE),
                _ => Err(StockoError::InvalidExchange),
            };
        }
        return Ok(Exchange::NYSE);
    }
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
struct Stock {
    symbol: String,
    exchange: Exchange,
    orders: Vec<Order>,

    #[serde(skip_serializing, default)]
    price: f64,
}

impl Stock {
    fn calculate_order_metrics(&self) -> OrderMetrics {
        let total_spent = self.orders
            .iter()
            .filter(|x| x.shares > 0)
            .fold(0.0, |acc, x| acc + x.shares as f64 * x.share_price);

        let total_sell = self.orders
            .iter()
            .filter(|x| x.shares < 0)
            .fold(0.0, |acc, x| acc + x.shares.abs() as f64 * x.share_price);

        let total_shares = self.orders.iter().fold(0, |acc, x| acc + x.shares);

        let average_price = total_spent / total_shares as f64;

        return OrderMetrics {
            total_spent,
            total_shares,
            average_price,
            total_sell,
        };
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Order {
    shares: i32,
    share_price: f64,
}

struct OrderMetrics {
    total_spent: f64,
    total_shares: i32,
    average_price: f64,
    total_sell: f64,
}

#[derive(Debug, Serialize, Deserialize)]
struct StockCollections {
    portfolio: HashMap<String, Stock>,
    watchlist: HashMap<String, Stock>,
    archive: HashMap<String, Stock>,
}

struct StockMetrics {
    change: f64,
    change_percentage: f64,
    close_today: f64,
    close_yesterday: f64,
}

impl StockCollections {
    fn new() -> StockCollections {
        return StockCollections {
            portfolio: HashMap::new(),
            watchlist: HashMap::new(),
            archive: HashMap::new(),
        };
    }

    fn print_watch_list(&self) -> Result<(), StockoError> {
        let mut table = Table::new();

        table.add_row(Row::new(vec![Cell::new_with_alignment(
            "Watch List",
            3,
            Alignment::Center,
        )]));

        table.add_row(Row::new(vec![
            Cell::new("Symbol", 1),
            Cell::new("Price", 1),
            Cell::new("Change", 1),
        ]));

        for stock in self.watchlist.values() {
            let time_series = fetch_symbol_time_series(&stock.symbol)?;
            let metrics = calculate_stock_metrics(time_series);

            let change = generate_change_string(&metrics);

            let row = Row::new(vec![
                Cell::new(stock.symbol.clone(), 1),
                Cell::new(metrics.close_today, 1),
                Cell::new(change, 1),
            ]);
            table.add_row(row);
        }

        println!("{}", table.as_string());

        Ok(())
    }

    fn print_portfolio(&self) -> Result<(), StockoError> {
        let mut table = Table::new();

        table.add_row(Row::new(vec![Cell::new_with_alignment(
            "Portfolio",
            6,
            Alignment::Center,
        )]));

        table.add_row(Row::new(vec![
            Cell::new("Symbol", 1),
            Cell::new("Price", 1),
            Cell::new("Change", 1),
            Cell::new("Shares", 1),
            Cell::new("Book Cost", 1),
            Cell::new("Total Gain", 1),
        ]));

        for stock in self.portfolio.values() {
            let time_series = fetch_symbol_time_series(&stock.symbol)?;
            let order_metrics = stock.calculate_order_metrics();
            let metrics = calculate_stock_metrics(time_series);
            let change = generate_change_string(&metrics);

            let overall_gain =
                (metrics.close_today - order_metrics.average_price) / order_metrics.average_price;

            let formatted_gain = if overall_gain >= 0.0 {
                Green
                    .paint(format!(
                        "+{:.2} (+{:.2}%)",
                        order_metrics.total_spent * overall_gain,
                        overall_gain * 100.0
                    ))
                    .to_string()
            } else {
                Red.paint(format!(
                    "{:.2} ({:.2}%)",
                    order_metrics.total_spent * (1.0 + overall_gain) - order_metrics.total_spent,
                    overall_gain * 100.0
                )).to_string()
            };

            let row = Row::new(vec![
                Cell::new(stock.symbol.clone(), 1),
                Cell::new(metrics.close_today, 1),
                Cell::new(change, 1),
                Cell::new(order_metrics.total_shares, 1),
                Cell::new(
                    order_metrics.total_shares as f64 * order_metrics.average_price,
                    1,
                ),
                Cell::new(formatted_gain, 1),
            ]);
            table.add_row(row);
        }

        println!("{}", table.as_string());

        Ok(())
    }

    fn print_archive(&self) -> Result<(), StockoError> {
        let mut table = Table::new();

        table.add_row(Row::new(vec![Cell::new_with_alignment(
            "Archive",
            3,
            Alignment::Center,
        )]));

        table.add_row(Row::new(vec![
            Cell::new("Symbol", 1),
            Cell::new("Orders", 1),
            Cell::new("Gain", 1),
        ]));

        let mut total_spent = 0.0;
        let mut total_sell = 0.0;

        for stock in self.archive.values() {
            let order_metrics = stock.calculate_order_metrics();

            let gain_percentage =
                (order_metrics.total_sell - order_metrics.total_spent) / order_metrics.total_spent;
            let overall_gain = order_metrics.total_sell - order_metrics.total_spent;

            total_spent += order_metrics.total_spent;
            total_sell += order_metrics.total_sell;

            let formatted_gain = generate_gain_string(overall_gain, gain_percentage);

            let mut orders = String::new();

            for order in &stock.orders {
                orders += &*format!("{} @ {}\n", order.shares, order.share_price);
            }
            orders.pop();

            let row = Row::new(vec![
                Cell::new(stock.symbol.clone(), 1),
                Cell::new(orders, 1),
                Cell::new(formatted_gain, 1),
            ]);
            table.add_row(row);
        }

        let total_gain_percentage = (total_sell - total_spent) / total_spent;
        let total_gain = total_sell - total_spent;

        let formatted_total_gain = generate_gain_string(total_gain, total_gain_percentage);

        table.add_row(Row::new(vec![
            Cell::new("Total Gain", 2),
            Cell::new(formatted_total_gain, 1),
        ]));

        println!("{}", table.as_string());

        Ok(())
    }
}

fn main() -> Result<(), StockoError> {
    let matches = App::new("managed-alias")
        .version("1.0")
        .author("Ryan Bluth")
        .subcommand(
            SubCommand::with_name("list")
                .alias("l")
                .about("Displays all stocks in portfolio"),
        )
        .subcommand(
            SubCommand::with_name("watch")
                .alias("w")
                .about("Displays all stocks in portfolio")
                .arg(
                    Arg::with_name("exchange")
                        .help("Exchange Symbol")
                        .index(2)
                        .required(false),
                )
                .arg(
                    Arg::with_name("symbol")
                        .help("Stock Symbol")
                        .index(1)
                        .required(true),
                ),
        )
        .subcommand(
            SubCommand::with_name("buy")
                .alias("b")
                .about("Add shares to your portfolio")
                .arg(
                    Arg::with_name("exchange")
                        .short("e")
                        .help("Exchange Symbol")
                        .takes_value(true)
                        .required(false),
                )
                .arg(
                    Arg::with_name("shares")
                        .short("s")
                        .help("Number of shares")
                        .takes_value(true)
                        .required(true),
                )
                .arg(
                    Arg::with_name("share_price")
                        .short("p")
                        .help("Share Price")
                        .takes_value(true)
                        .required(true),
                )
                .arg(
                    Arg::with_name("symbol")
                        .help("Stock Symbol")
                        .index(1)
                        .required(true),
                ),
        )
        .subcommand(
            SubCommand::with_name("sell")
                .alias("s")
                .about("Remove shares to your portfolio")
                .arg(
                    Arg::with_name("exchange")
                        .short("e")
                        .help("Exchange Symbol")
                        .takes_value(true)
                        .required(false),
                )
                .arg(
                    Arg::with_name("shares")
                        .short("s")
                        .help("Number of shares")
                        .takes_value(true)
                        .required(true),
                )
                .arg(
                    Arg::with_name("share_price")
                        .short("p")
                        .help("Share Price")
                        .takes_value(true)
                        .required(true),
                )
                .arg(
                    Arg::with_name("symbol")
                        .help("Stock Symbol")
                        .index(1)
                        .required(true),
                ),
        )
        .get_matches();

    if matches.subcommand_matches("list").is_some() {
        list()?;
    } else if let Some(sub_matches) = matches.subcommand_matches("watch") {
        let mut symbol = String::from(sub_matches.value_of("symbol").unwrap());
        let exchange_value = sub_matches.value_of("exchange");
        if let Some(exchange_symbol) = sub_matches.value_of("exchange") {
            let suffix = suffix_for_exchange_symbol(exchange_symbol)?;
            symbol.push_str(suffix);
        }
        watch(symbol, exchange_value)?;
    } else if matches.subcommand_matches("buy").is_some()
        || matches.subcommand_matches("sell").is_some()
    {
        let sub_matches = matches
            .subcommand_matches("buy")
            .unwrap_or_else(|| matches.subcommand_matches("sell").unwrap());
        let mut symbol = String::from(sub_matches.value_of("symbol").unwrap());
        let exchange_value = sub_matches.value_of("exchange");
        if let Some(exchange_symbol) = sub_matches.value_of("exchange") {
            let suffix = suffix_for_exchange_symbol(exchange_symbol)?;
            symbol.push_str(suffix);
        }
        let mut shares = value_t!(sub_matches, "shares", i32).unwrap();
        let price = value_t!(sub_matches, "share_price", f64).unwrap();
        if matches.subcommand_matches("sell").is_some() {
            shares *= -1;
        }
        process_order(symbol.to_uppercase(), exchange_value, shares, price)?;
    }
    Ok(())
}

fn watch(symbol: String, exchange_symbol: Option<&str>) -> Result<(), StockoError> {
    let mut collections = load_data()?;
    // Run a fetch to make sure things are working
    fetch_symbol_time_series(symbol.as_str())?;
    let stock = Stock {
        exchange: Exchange::from_symbol(exchange_symbol)?,
        symbol: symbol.clone().to_uppercase(),
        orders: Vec::new(),
        ..Default::default()
    };

    collections
        .watchlist
        .insert(symbol.clone().to_uppercase(), stock);
    save_data(collections)?;
    Ok(())
}

fn process_order(
    symbol: String,
    exchange_symbol: Option<&str>,
    shares: i32,
    price: f64,
) -> Result<(), StockoError> {
    let mut collection = load_data()?;
    if !collection.portfolio.contains_key(&symbol) {
        println!("{:?}", collection.portfolio);
        if shares > 0 {
            collection.portfolio.insert(
                symbol.clone().to_uppercase(),
                Stock {
                    symbol: symbol.clone().to_uppercase(),
                    exchange: Exchange::from_symbol(exchange_symbol)?,
                    orders: Vec::new(),
                    ..Default::default()
                },
            );
        } else {
            return Err(StockoError::InvalidShareQuantity {
                symbol: symbol,
                shares: shares.abs() as u32,
            });
        }
    }

    let mut stock = collection.portfolio.get(&symbol).unwrap().clone();

    let total_shares = stock.calculate_order_metrics().total_shares;

    println!("{}", total_shares);

    if shares < 0 && total_shares < shares.abs() {
        return Err(StockoError::InvalidShareQuantity {
            symbol: symbol,
            shares: shares.abs() as u32,
        });
    }

    let order = Order {
        shares: shares,
        share_price: price,
    };

    stock.orders.push(order);

    if shares < 0 && total_shares == shares.abs() {
        collection.portfolio.remove(&symbol);
        collection.archive.insert(symbol, stock);
    } else {
        collection.portfolio.insert(symbol, stock);
    }

    save_data(collection)?;

    Ok(())
}

fn fetch_symbol_time_series(symbol: &str) -> Result<TimeSeries, StockoError> {
    let client = alphavantage::Client::new("BUN9HP4GJXX524JS");
    let time_series = mapStockoErr!(
        StockoError::AlphaVantageError,
        client.get_time_series_daily(symbol)
    )?;

    return Ok(time_series);
}

fn list() -> Result<(), StockoError> {
    let collection = load_data()?;
    collection.print_portfolio()?;
    collection.print_watch_list()?;
    collection.print_archive()?;
    Ok(())
}

fn save_data(collections: StockCollections) -> Result<(), StockoError> {
    let mut file = mapStockoErr!(
        StockoError::SaveDataError,
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .read(true)
            .open(get_data_file_path())
    )?;

    let json = mapStockoErr!(StockoError::SaveDataError, serde_json::to_vec(&collections))?;

    return mapStockoErr!(StockoError::SaveDataError, file.write_all(&*json));
}

fn load_data() -> Result<StockCollections, StockoError> {
    let path = get_data_file_path();

    if !path.exists() {
        return Ok(StockCollections::new());
    }

    let mut file = mapStockoErr!(StockoError::ReadDataError, File::open(path))?;

    let mut buf = String::new();
    file.read_to_string(&mut buf).unwrap();

    return mapStockoErr!(
        StockoError::ReadDataError,
        serde_json::from_str::<StockCollections>(buf.as_str())
    );
}

fn get_data_file_path() -> PathBuf {
    let mut exe_path = std::env::current_exe().unwrap();
    exe_path.pop();
    exe_path.push("stocko_data.json");
    return exe_path;
}

fn suffix_for_exchange_symbol(exchange_symbol: &str) -> Result<&'static str, StockoError> {
    match exchange_symbol.to_lowercase().as_ref() {
        "tsx" => Ok(".TO"),
        "tsxv" => Ok(".V"),
        "nsye" => Ok(""),
        _ => Err(StockoError::InvalidExchange),
    }
}

fn calculate_stock_metrics(time_series: TimeSeries) -> StockMetrics {
    let entries = time_series.entries();
    let num_entries = entries.len();
    let mut entry_iter = entries.into_iter();

    let (_date_yesterday, entry_yesterday) = entry_iter.nth(num_entries - 2).unwrap();
    let (_date_today, entry_today) = entry_iter.last().unwrap();

    let change_value = entry_today.close - entry_yesterday.close;
    let change_percentage =
        100.0 * (entry_today.close - entry_yesterday.close) / entry_yesterday.close;

    return StockMetrics {
        change_percentage: change_percentage,
        change: change_value,
        close_today: entry_today.close,
        close_yesterday: entry_yesterday.close,
    };
}

fn generate_change_string(metrics: &StockMetrics) -> String {
    return if metrics.change >= 0.0 {
        Green
            .paint(format!(
                "+{:.2} (+{:.2}%)",
                metrics.change, metrics.change_percentage
            ))
            .to_string()
    } else {
        Red.paint(format!(
            "{:.2} ({:.2}%)",
            metrics.change, metrics.change_percentage
        )).to_string()
    };
}

fn generate_gain_string(gain: f64, gain_percentage: f64) -> String {
    return if gain >= 0.0 {
        Green
            .paint(format!("+{:.2} (+{:.2}%)", gain, gain_percentage * 100.0))
            .to_string()
    } else {
        Red.paint(format!("{:.2} ({:.2}%)", gain, gain_percentage * 100.0))
            .to_string()
    };
}
