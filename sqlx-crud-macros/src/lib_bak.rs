use inflector::Inflector;
use proc_macro::{self, TokenStream};
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::punctuated::Punctuated;
use syn::token::Comma;
use syn::{
    parse_macro_input, Attribute, Data, DataStruct, DeriveInput, Field, Fields, FieldsNamed, Ident,
    Lit, LitStr, Meta, MetaNameValue,
};

#[allow(dead_code)] // Usage in quote macros aren't flagged as used
struct Config<'a> {
    ident: &'a Ident,
    named: &'a Punctuated<Field, Comma>,
    crate_name: TokenStream2,
    db_ty: DbType,
    model_schema_ident: Ident,
    table_name: String,
    id_column_ident: Ident,
    external_id: bool,
}

#[proc_macro_derive(SqlxCrud, attributes(database, id))]
pub fn derive(input: TokenStream) -> TokenStream {
    let DeriveInput {
        ident, data, attrs, ..
    } = parse_macro_input!(input);
    match data {
        Data::Struct(DataStruct {
            fields: Fields::Named(FieldsNamed { named, .. }),
            ..
        }) => {
            let config = Config::new(&attrs, &ident, &named);
            let crate_name = &config.crate_name;
            let static_model_schema = build_static_model_schema(&config);
            let sqlx_crud_impl = build_sqlx_crud_impl(&config);

            quote! {
                #[automatically_derived]
                use #crate_name::traits::{Crud, Schema};

                #static_model_schema

                //#sqlx_crud_impl
            }
            .into()
        }
        _ => panic!("this derive macro only works on structs with named fields"),
    }
}

fn build_static_model_schema(config: &Config) -> TokenStream2 {
    let crate_name = &config.crate_name;
    let model_schema_ident = &config.model_schema_ident;
    let table_name = &config.table_name;
    let id_column_ident = &config.id_column_ident;

    let columns_len = config.named.iter().count();
    let columns = config.named
        .iter()
        .flat_map(|f| &f.ident)
        .map(|f| LitStr::new(format!("{}", f).as_str(), f.span()));

    let sql_queries = build_sql_queries(&config);

    quote! {
        #[automatically_derived]
        static #model_schema_ident: #crate_name::schema::Metadata<'static, #columns_len> = #crate_name::schema::Metadata {
            table_name: #table_name,
            id_column: #id_column_ident,
            columns: [#(#columns),*],
            #sql_queries
        };
    }
}

fn build_sql_queries(config: &Config) -> TokenStream2 {
    let table_name = config.quote_ident(&config.table_name);
    let id_column = format!(
        "{}.{}",
        &table_name,
        config.quote_ident(&config.id_column_ident.to_string())
    );
    let insert_bind_cnt = if config.external_id {
        config.named.iter().count()
    } else {
        config.named.iter().count() - 1
    };
    let insert_sql_binds = (0..insert_bind_cnt)
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");
    let update_sql_binds = config.named
        .iter()
        .flat_map(|f| &f.ident)
        .filter(|i| *i != &config.id_column_ident)
        .map(|i| format!("{} = ?", config.quote_ident(&i.to_string())))
        .collect::<Vec<_>>()
        .join(", ");

    let insert_column_list = config.named
        .iter()
        .flat_map(|f| &f.ident)
        .filter(|i| !config.external_id && *i != &config.id_column_ident)
        .map(|i| config.quote_ident(&i.to_string()))
        .collect::<Vec<_>>()
        .join(", ");
    let column_list = config.named
        .iter()
        .flat_map(|f| &f.ident)
        .map(|i| format!("{}.{}", &table_name, config.quote_ident(&i.to_string())))
        .collect::<Vec<_>>()
        .join(", ");

    let select_sql = format!("SELECT {} FROM {}", &column_list, &table_name);
    let select_by_id_sql = format!(
        "SELECT {} FROM {} WHERE {} = ? LIMIT 1",
        &column_list, &table_name, &id_column
    );
    let insert_sql = format!(
        "INSERT INTO {} ({}) VALUES ({}) RETURNING {}",
        &table_name, &insert_column_list, &insert_sql_binds, &column_list
    );
    let update_by_id_sql = format!(
        "UPDATE {} SET {} WHERE {} = ? RETURNING {}",
        &table_name, &update_sql_binds, &id_column, &column_list
    );
    let delete_by_id_sql = format!("DELETE FROM {} WHERE {} = ?", &table_name, &id_column);

    quote! {
        select_sql: #select_sql,
        select_by_id_sql: #select_by_id_sql,
        insert_sql: #insert_sql,
        update_by_id_sql: #update_by_id_sql,
        delete_by_id_sql: #delete_by_id_sql,
    }
}

fn build_sqlx_crud_impl(config: &Config) -> TokenStream2 {
    let crate_name = &config.crate_name;
    let ident = &config.ident;
    let model_schema_ident = &config.model_schema_ident;
    let id_column_ident = &config.id_column_ident;

    let id_ty = config.named
        .iter()
        .find(|f| f.ident.as_ref() == Some(&config.id_column_ident))
        .map(|f| &f.ty)
        .expect("the id type");

    let insert_binds = config.named
        .iter()
        .flat_map(|f| &f.ident)
        .map(|i| quote! { .bind(&self.#i) });
    let update_binds = config.named
        .iter()
        .flat_map(|f| &f.ident)
        .filter(|i| *i != &config.id_column_ident)
        .map(|i| quote! { .bind(&self.#i) });

    let sqlx_db = config.db_ty.build_sqlx_db();

    quote! {
        #[automatically_derived]
        impl #crate_name::traits::Schema for #ident {
            type Id = #id_ty;

            fn table_name() -> &'static str {
                #model_schema_ident.table_name
            }

            fn id(&self) -> Self::Id {
                self.#id_column_ident
            }

            fn id_column() -> &'static str {
                #model_schema_ident.id_column
            }

            fn columns() -> &'static [&'static str] {
                &#model_schema_ident.columns
            }

            fn select_sql() -> &'static str {
                #model_schema_ident.select_sql
            }

            fn select_by_id_sql() -> &'static str {
                #model_schema_ident.select_by_id_sql
            }

            fn insert_sql() -> &'static str {
                #model_schema_ident.insert_sql
            }

            fn update_by_id_sql() -> &'static str {
                #model_schema_ident.update_by_id_sql
            }

            fn delete_by_id_sql() -> &'static str {
                #model_schema_ident.delete_by_id_sql
            }
        }

        #[automatically_derived]
        impl<'e> #crate_name::traits::Crud<'e, &'e ::sqlx::pool::Pool<#sqlx_db>> for #ident {
            fn insert_binds(
                &'e self,
                query: ::sqlx::query::Query<'e, ::sqlx::Sqlite, ::sqlx::sqlite::SqliteArguments<'e>>
            ) -> ::sqlx::query::Query<'e, ::sqlx::Sqlite, ::sqlx::sqlite::SqliteArguments<'e>> {
                query
                    #(#insert_binds)*
            }

            fn update_binds(
                &'e self,
                query: ::sqlx::query::Query<'e, ::sqlx::Sqlite, ::sqlx::sqlite::SqliteArguments<'e>>
            ) -> ::sqlx::query::Query<'e, ::sqlx::Sqlite, ::sqlx::sqlite::SqliteArguments<'e>> {
                query
                    #(#update_binds)*
                    .bind(&self.#id_column_ident)
            }
        }
    }
}

impl<'a> Config<'a> {
    fn new(
        attrs: &[Attribute],
        ident: &'a Ident,
        named: &'a Punctuated<Field, Comma>,
    ) -> Self {
        let crate_name = std::env::var("CARGO_PKG_NAME").unwrap();
        let is_doctest = std::env::vars()
            .any(|(k, _)| k == "UNSTABLE_RUSTDOC_TEST_LINE" || k == "UNSTABLE_RUSTDOC_TEST_PATH");
        let crate_name = if !is_doctest && crate_name == "sqlx-crud" {
            quote! { crate }
        } else {
            quote! { ::sqlx_crud }
        };

        let db_ty = DbType::from_attributes(&attrs);

        let model_schema_ident = format_ident!("{}_SCHEMA", ident.to_string().to_screaming_snake_case());

        let table_name = ident.to_string().to_table_case();

        // Search for a field with the #[id] attribute
        let id_attr = &named
            .iter()
            .find(|f| f.attrs.iter().any(|a| a.path.is_ident("id")))
            .map(|f| f.ident.as_ref())
            .flatten();
        // Otherwise default to the first field as the "id" column
        let id_column_ident = id_attr.unwrap_or_else(|| {
            named
                .iter()
                .flat_map(|f| &f.ident)
                .next()
                .expect("the first field")
        }).clone();

        let external_id = match attrs
            .iter()
            .find(|a| a.path.is_ident("external_ids"))
            .map(|a| a.parse_meta())
        {
            Some(Ok(Meta::NameValue(MetaNameValue {
                lit: Lit::Str(s), ..
            }))) => s.value().parse::<bool>().unwrap_or_else(|_| false),
            _ => false,
        };

        Self {
            ident,
            named,
            crate_name,
            db_ty,
            model_schema_ident,
            table_name,
            id_column_ident,
            external_id,
        }
    }

    fn quote_ident(&self, ident: &str) -> String {
        self.db_ty.quote_ident(&ident)
    }
}

enum DbType {
    Any,
    Mssql,
    MySql,
    Postgres,
    Sqlite,
}

impl From<&str> for DbType {
    fn from(db_type: &str) -> Self {
        match db_type {
            "Any" => Self::Any,
            "Mssql" => Self::Mssql,
            "MySql" => Self::MySql,
            "Postgres" => Self::Postgres,
            "Sqlite" => Self::Sqlite,
            _ => panic!("unknown #[database] type {}", db_type),
        }
    }
}

impl DbType {
    fn from_attributes(attrs: &[Attribute]) -> Self {
        match attrs
            .iter()
            .find(|a| a.path.is_ident("database"))
            .map(|a| a.parse_meta())
        {
            Some(Ok(Meta::NameValue(MetaNameValue {
                lit: Lit::Str(s), ..
            }))) => DbType::from(&*s.value()),
            _ => Self::Sqlite,
        }
    }

    fn build_sqlx_db(&self) -> TokenStream2 {
        match self {
            Self::Any => quote! { ::sqlx::Any },
            Self::Mssql => quote! { ::sqlx::Mssql },
            Self::MySql => quote! { ::sqlx::MySql },
            Self::Postgres => quote! { ::sqlx::Postgres },
            Self::Sqlite => quote! { ::sqlx::Sqlite },
        }
    }

    fn quote_ident(&self, ident: &str) -> String {
        match self {
            Self::Any => format!(r#""{}""#, &ident),
            Self::Mssql => format!(r#""{}""#, &ident),
            Self::MySql => format!("`{}`", &ident),
            Self::Postgres => format!(r#""{}""#, &ident),
            Self::Sqlite => format!(r#""{}""#, &ident),
        }
    }
}