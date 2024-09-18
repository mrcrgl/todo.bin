use anyhow::anyhow;
use chrono::Utc;
use clap::{Parser, Subcommand};
use handlebars::{DirectorySourceOptions, Handlebars};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env::current_dir;
use std::fmt::Display;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tokio::runtime::Handle;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let current_dir = cli.data_dir.unwrap_or(current_dir().unwrap());

    if !current_dir.is_absolute() {
        failure("Current directory is not absolute.");
    }

    match cli.command {
        None => {}
        Some(Commands::New {
            template,
            title,
            tags,
        }) => {
            let proc = CommandProcessor::new(
                init_hbs().unwrap(),
                load_collection().await.unwrap(),
                current_dir,
            );
            let mut template_vars = TemplateVars::new(proc.next_data_id());
            template_vars.title = title;
            template_vars.tags = tags;
            let todo_file_result = proc.new_todo_from_template(
                template.unwrap_or("task".to_string()).as_str(),
                template_vars,
            );

            let todo_file = match todo_file_result {
                Ok(todo_file) => todo_file,
                Err(err) => {
                    failure(err);
                }
            };

            if let Err(err) = todo_file.write_file().await {
                failure(err);
            }

            println!(
                "{} {}",
                todo_file
                    .path
                    .strip_prefix(proc.data_dir)
                    .unwrap()
                    .to_str()
                    .unwrap(),
                todo_file.path.file_name().unwrap().to_str().unwrap()
            )
        }

        Some(Commands::Init) => {
            let proc = CommandProcessor::new(
                Handlebars::new(),
                Collection::new(),
                current_dir,
            );

            if let Err(err) = proc.init().await {
                failure(err);
            }
        }
    }
}

fn failure(err: impl Display) -> ! {
    eprintln!("Error: {err}");
    std::process::exit(1);
}

#[derive(Parser)]
#[command(version, about, long_about = None)]
#[command(arg_required_else_help = true)]
struct Cli {
    #[arg(long)]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// does testing things
    New {
        /// template to use
        #[arg(long)]
        template: Option<String>,

        /// title
        #[arg(long)]
        title: Option<String>,

        /// tags
        #[arg(long = "tag", short)]
        tags: Vec<String>,
    },
    /// Initialize directory for todo
    Init,
}

struct CommandProcessor<'a> {
    data_dir: PathBuf,
    tasks_dir: PathBuf,
    templates_dir: PathBuf,
    hbs: Handlebars<'a>,
    collection: Collection,
}

impl<'a> CommandProcessor<'a> {
    pub fn new(
        hbs: Handlebars<'a>,
        collection: Collection,
        data_dir: PathBuf,
    ) -> CommandProcessor<'a> {
        let tasks_dir = data_dir.join("tasks");
        let templates_dir = data_dir.join("templates");
        Self {
            hbs,
            collection,
            data_dir,
            tasks_dir,
            templates_dir,
        }
    }
}
impl CommandProcessor<'_> {
    pub fn next_data_id(&self) -> DataId {
        self.collection
            .keys()
            .max()
            .map_or_else(|| 1, |last| last + 1)
    }

    pub fn new_todo_from_template(
        &self,
        template: &str,
        template_vars: TemplateVars,
    ) -> anyhow::Result<TodoFile> {
        Ok(TodoFile::new_from_data(
            self.create_todo_data_from_template(template, template_vars)?,
        ))
    }

    fn create_todo_data_from_template(
        &self,
        template: &str,
        template_vars: TemplateVars,
    ) -> anyhow::Result<TodoData> {
        let foo = self.hbs.render(template, &template_vars)?;
        Ok(TodoData::from_str(foo.as_str())
            .map_err(|err| anyhow!("invalid template '{template}': {err:?}"))?)
    }

    pub async fn is_initialized(&self) -> anyhow::Result<bool> {
        if !tokio::fs::try_exists(self.tasks_dir.as_path()).await? {
            return Ok(false);
        }
        if !tokio::fs::try_exists(self.templates_dir.as_path()).await? {
            return Ok(false);
        }
        Ok(true)
    }

    pub async fn init(&self) -> anyhow::Result<()> {
        if self.is_initialized().await? {
            return Err(anyhow!("directories tasks and/or templates already exists"));
        }

        tokio::fs::create_dir_all(self.tasks_dir.as_path()).await?;
        tokio::fs::create_dir_all(self.templates_dir.as_path()).await?;
        tokio::fs::write(
            self.templates_dir.join("task.md.hbs").as_path(),
            TASK_TEMPLATE,
        )
        .await?;

        Ok(())
    }
}

#[derive(Debug)]
struct TodoFile {
    path: PathBuf,
    data: TodoData,
}

impl TodoFile {
    pub async fn load_file(path: &Path) -> anyhow::Result<Self> {
        let content = tokio::fs::read_to_string(path).await?;

        Ok(Self {
            path: path.to_path_buf(),
            data: TodoData::from_str(content.as_str())?,
        })
    }

    pub async fn write_file(&self) -> anyhow::Result<()> {
        tokio::fs::write(self.path.as_path(), self.data.to_bytes()).await?;
        Ok(())
    }

    fn gen_filepath(id: DataId) -> PathBuf {
        Path::new(std::env::current_dir().unwrap().as_path())
            .join("tasks")
            .join(format!("{:010}.todo.md", id))
    }

    pub fn new_from_data(todo_data: TodoData) -> Self {
        Self {
            path: Self::gen_filepath(todo_data.front_matter.id),
            data: todo_data,
        }
    }
}

#[derive(Debug)]
struct TodoData {
    front_matter: FrontMatter,
    content: String,
}

impl TodoData {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = BufWriter::new(Vec::new());

        write!(&mut buf, "+++\n").unwrap();
        write!(
            &mut buf,
            "{}\n",
            toml::to_string(&self.front_matter).unwrap()
        )
        .unwrap();
        write!(&mut buf, "+++\n").unwrap();
        write!(&mut buf, "{}", self.content).unwrap();

        buf.into_inner().unwrap()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct FrontMatter {
    id: DataId,
    created_at: chrono::DateTime<chrono::Utc>,
    due_at: Option<chrono::DateTime<chrono::Utc>>,
    tags: Vec<String>,
}

type DataId = u32;
type Collection = HashMap<DataId, TodoFile>;

async fn load_collection() -> anyhow::Result<Collection> {
    let mut cur_dir = tokio::fs::read_dir(std::env::current_dir()?.join("tasks")).await?;
    let mut connection = Collection::new();

    while let Some(entry) = cur_dir.next_entry().await? {
        if !entry.file_type().await?.is_file() {
            continue;
        }

        if !entry.path().extension().is_some_and(|ext| ext.eq("md")) {
            continue;
        }

        if let Ok(file) = TodoFile::load_file(entry.path().as_path()).await {
            if connection.insert(file.data.front_matter.id, file).is_some() {
                return Err(anyhow!("duplicate content id"));
            }
        }
    }

    Ok(connection)
}

impl FromStr for TodoData {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mat = s.splitn(3, "+++\n");

        let parts: Vec<_> = mat.collect();
        if parts.is_empty() {
            return Err(anyhow::anyhow!("invalid content"));
        }

        let data = TodoData {
            front_matter: toml::from_str(parts.get(1).unwrap())?,
            content: parts.get(2).unwrap().to_string(),
        };

        Ok(data)
    }
}

fn init_hbs() -> anyhow::Result<Handlebars<'static>> {
    let mut options = DirectorySourceOptions::default();
    options.tpl_extension = ".md.hbs".to_string();
    options.temporary = false;

    let mut hbs = Handlebars::new();
    hbs.register_templates_directory(std::env::current_dir()?.join("templates"), options)?;

    Ok(hbs)
}

#[derive(Serialize)]
struct TemplateVars {
    id: DataId,
    created_at: chrono::DateTime<chrono::Utc>,
    tags: Vec<String>,
    title: Option<String>,
}

impl TemplateVars {
    fn new(id: DataId) -> Self {
        Self {
            id,
            created_at: Utc::now(),
            tags: vec![],
            title: None,
        }
    }
}

const TASK_TEMPLATE: &str = r#"+++
id = {{ id }}
created_at = "{{ created_at }}"
tags = [ {{#each tags}}{{#if @index}}, {{/if}}"{{@index}} {{this}}"{{/each}} ]
+++

# {{#if title}}{{title}}{{else}}Title{{/if}}

"#;
