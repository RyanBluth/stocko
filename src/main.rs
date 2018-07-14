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
use std::fmt::{Formatter, Debug};
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::io::{Read, Write};

use alphavantage::time_series::TimeSeries;

use clap::{App, Arg, SubCommand};

use ansi_term::Colour::{Red, Green};

type StockMap = HashMap<String, HashMap<String, Stock>>;

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
            return match symbol {
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
        let total_spent = self
            .orders
            .iter()
            .fold(0.0, |acc, x| acc + x.shares as f64 * x.share_price);

        let total_shares = self.orders.iter().fold(0, |acc, x| acc + x.shares);
        let average_price = total_spent / total_shares as f64;

        return OrderMetrics {
            total_spent,
            total_shares,
            average_price,
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
        process_order(symbol, exchange_value, shares, price)?;
    }
    Ok(())
}

fn watch(symbol: String, exchange_symbol: Option<&str>) -> Result<(), StockoError> {
    let mut collection = load_data()?;
    // Run a fetch to make sure things are working
    fetch_symbol_time_series(symbol.as_str())?;
    let stock = Stock {
        exchange: Exchange::from_symbol(exchange_symbol)?,
        symbol: symbol.clone().to_uppercase(),
        orders: Vec::new(),
        ..Default::default()
    };
    let key = String::from("Watch List");

    collection
        .get_mut(&key)
        .unwrap()
        .insert(symbol.clone().to_uppercase(), stock);
    save_data(collection)?;
    Ok(())
}

fn process_order(
    symbol: String,
    exchange_symbol: Option<&str>,
    shares: i32,
    price: f64,
) -> Result<(), StockoError> {
    let mut collection = load_data()?;
    let mut stocks = collection.get(&String::from("Portfolio")).unwrap().clone();

    if shares > 0 && !stocks.contains_key(&symbol) {
        stocks.insert(
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

    {
        let stock = stocks.get_mut(&symbol).unwrap();

        let total_shares = stock.calculate_order_metrics().total_shares;

        if total_shares < shares {
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
    }

    collection.insert("Portfolio".to_string(), stocks);

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

    for key in collection.keys() {
        let mut table = Table::new();
        
        if key.eq(&String::from("Portfolio")) {
            table.add_row(Row::new(vec![Cell::new_with_alignment(
                key.as_str(),
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
        } else {
            table.add_row(Row::new(vec![Cell::new_with_alignment(
                key.as_str(),
                3,
                Alignment::Center,
            )]));

            table.add_row(Row::new(vec![
                Cell::new("Symbol", 1),
                Cell::new("Price", 1),
                Cell::new("Change", 1),
            ]));
        }

        for stock in collection.get(key).unwrap().values() {
            let time_series = fetch_symbol_time_series(&stock.symbol)?;
            let entries = time_series.entries();
            let num_entries = entries.len();
            let mut entry_iter = entries.into_iter();

            let (_date_yesterday, entry_yesterday) = entry_iter.nth(num_entries - 2).unwrap();
            let (_date_today, entry_today) = entry_iter.last().unwrap();

            let change_value = entry_today.close - entry_yesterday.close;
            let change_percentage =
                100.0 * (entry_today.close - entry_yesterday.close) / entry_yesterday.close;

            let change = if change_value >= 0.0 {
                Green.paint(format!("+{:.2} (+{:.2}%)", change_value, change_percentage)).to_string()
            } else {
                Red.paint(format!("{:.2} ({:.2}%)", change_value, change_percentage)).to_string()
            };

            if key.eq(&String::from("Portfolio")) {
                let order_metrics = stock.calculate_order_metrics();

                let overall_gain =
                    (order_metrics.average_price - entry_today.close) / order_metrics.average_price;

                let formatted_gain = if overall_gain >= 0.0 {
                    Green.paint(format!(
                        "+{:.2} (+{:.2}%)",
                        order_metrics.total_spent * (1.0 + overall_gain),
                        overall_gain * 100.0
                    )).to_string()
                } else {
                    Red.paint(format!(
                        "{:.2} ({:.2}%)",
                        order_metrics.total_spent * (1.0 + overall_gain)
                            - order_metrics.total_spent,
                        overall_gain * 100.0
                    )).to_string()
                };

                let row = Row::new(vec![
                    Cell::new(stock.symbol.clone(), 1),
                    Cell::new(entry_today.close, 1),
                    Cell::new(change, 1),
                    Cell::new(order_metrics.total_shares, 1),
                    Cell::new(
                        order_metrics.total_shares as f64 * order_metrics.average_price,
                        1,
                    ),
                    Cell::new(formatted_gain, 1),
                ]);
                table.add_row(row);
            } else {
                let row = Row::new(vec![
                    Cell::new(stock.symbol.clone(), 1),
                    Cell::new(entry_today.close, 1),
                    Cell::new(change, 1),
                ]);
                table.add_row(row);
            }
        }
        println!("{}", table.as_string());
    }

    Ok(())
}

fn save_data(collections: StockMap) -> Result<(), StockoError> {
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

fn load_data() -> Result<StockMap, StockoError> {
    let path = get_data_file_path();

    if !path.exists() {
        return Ok(gen_default_stock_collections());
    }

    let mut file = mapStockoErr!(StockoError::ReadDataError, File::open(path))?;

    let mut buf = String::new();
    file.read_to_string(&mut buf).unwrap();

    return mapStockoErr!(
        StockoError::ReadDataError,
        serde_json::from_str::<StockMap>(buf.as_str())
    );
}

fn get_data_file_path() -> PathBuf {
    let mut exe_path = std::env::current_exe().unwrap();
    exe_path.pop();
    exe_path.push("stocko_data.json");
    return exe_path;
}

fn gen_default_stock_collections() -> StockMap {
    let mut res = StockMap::new();
    res.insert(String::from("Watch List"), HashMap::new());
    res.insert(String::from("Portfolio"), HashMap::new());
    return res;
}

fn suffix_for_exchange_symbol(exchange_symbol: &str) -> Result<&'static str, StockoError> {
    match exchange_symbol.to_lowercase().as_ref() {
        "tsx" => Ok(".TO"),
        "tsxv" => Ok(".V"),
        "nsye" => Ok(""),
        _ => Err(StockoError::InvalidExchange),
    }
}
