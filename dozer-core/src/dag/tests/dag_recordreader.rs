#![allow(non_snake_case)]
use crate::dag::channels::ProcessorChannelForwarder;
use crate::dag::dag::{Dag, Endpoint, NodeType};
use crate::dag::errors::ExecutionError;
use crate::dag::executor_local::{ExecutorOptions, MultiThreadedDagExecutor};
use crate::dag::node::{
    NodeHandle, OutputPortDef, OutputPortDefOptions, PortHandle, Processor, ProcessorFactory,
};
use crate::dag::record_store::RecordReader;
use crate::dag::tests::sinks::{CountingSinkFactory, COUNTING_SINK_INPUT_PORT};
use crate::dag::tests::sources::{
    GeneratorSourceFactory, StatefulGeneratorSourceFactory, GENERATOR_SOURCE_OUTPUT_PORT,
};
use crate::storage::common::{Environment, RwTransaction};
use dozer_types::types::{Field, Operation, Schema};
use std::collections::HashMap;
use std::fs;

use log::info;
use std::time::Duration;
use tempdir::TempDir;

macro_rules! chk {
    ($stmt:expr) => {
        $stmt.unwrap_or_else(|e| panic!("{}", e.to_string()))
    };
}

pub(crate) const PASSTHROUGH_PROCESSOR_INPUT_PORT: PortHandle = 50;
pub(crate) const PASSTHROUGH_PROCESSOR_OUTPUT_PORT: PortHandle = 60;

pub(crate) struct PassthroughProcessorFactory {}

impl PassthroughProcessorFactory {
    pub fn new() -> Self {
        Self {}
    }
}

impl ProcessorFactory for PassthroughProcessorFactory {
    fn get_input_ports(&self) -> Vec<PortHandle> {
        vec![PASSTHROUGH_PROCESSOR_INPUT_PORT]
    }
    fn get_output_ports(&self) -> Vec<OutputPortDef> {
        vec![OutputPortDef::new(
            PASSTHROUGH_PROCESSOR_OUTPUT_PORT,
            OutputPortDefOptions::new(true, true, true),
        )]
    }
    fn build(&self) -> Box<dyn Processor> {
        Box::new(PassthroughProcessor {})
    }
}

pub(crate) struct PassthroughProcessor {}

impl Processor for PassthroughProcessor {
    fn update_schema(
        &mut self,
        _output_port: PortHandle,
        input_schemas: &HashMap<PortHandle, Schema>,
    ) -> Result<Schema, ExecutionError> {
        Ok(input_schemas
            .get(&PASSTHROUGH_PROCESSOR_INPUT_PORT)
            .unwrap()
            .clone())
    }

    fn init(&mut self, _tx: &mut dyn Environment) -> Result<(), ExecutionError> {
        Ok(())
    }

    fn process(
        &mut self,
        _from_port: PortHandle,
        op: Operation,
        fw: &mut dyn ProcessorChannelForwarder,
        _tx: &mut dyn RwTransaction,
        _readers: &HashMap<PortHandle, RecordReader>,
    ) -> Result<(), ExecutionError> {
        fw.send(op, PASSTHROUGH_PROCESSOR_OUTPUT_PORT)
    }

    fn commit(&self, _tx: &mut dyn RwTransaction) -> Result<(), ExecutionError> {
        info!("Commit notification");
        Ok(())
    }
}

pub(crate) struct RecordReaderProcessorFactory {}

impl RecordReaderProcessorFactory {
    pub fn new() -> Self {
        Self {}
    }
}

pub(crate) const RECORD_READER_PROCESSOR_INPUT_PORT: PortHandle = 70;
pub(crate) const RECORD_READER_PROCESSOR_OUTPUT_PORT: PortHandle = 80;

impl ProcessorFactory for RecordReaderProcessorFactory {
    fn get_input_ports(&self) -> Vec<PortHandle> {
        vec![RECORD_READER_PROCESSOR_INPUT_PORT]
    }
    fn get_output_ports(&self) -> Vec<OutputPortDef> {
        vec![OutputPortDef::new(
            RECORD_READER_PROCESSOR_OUTPUT_PORT,
            OutputPortDefOptions::default(),
        )]
    }
    fn build(&self) -> Box<dyn Processor> {
        Box::new(RecordReaderProcessor { ctr: 0 })
    }
}

pub(crate) struct RecordReaderProcessor {
    ctr: u64,
}

impl Processor for RecordReaderProcessor {
    fn update_schema(
        &mut self,
        _output_port: PortHandle,
        input_schemas: &HashMap<PortHandle, Schema>,
    ) -> Result<Schema, ExecutionError> {
        Ok(input_schemas
            .get(&RECORD_READER_PROCESSOR_INPUT_PORT)
            .unwrap()
            .clone())
    }

    fn init(&mut self, _tx: &mut dyn Environment) -> Result<(), ExecutionError> {
        Ok(())
    }

    fn process(
        &mut self,
        _from_port: PortHandle,
        op: Operation,
        fw: &mut dyn ProcessorChannelForwarder,
        _tx: &mut dyn RwTransaction,
        readers: &HashMap<PortHandle, RecordReader>,
    ) -> Result<(), ExecutionError> {
        let v = readers
            .get(&RECORD_READER_PROCESSOR_INPUT_PORT)
            .unwrap()
            .get(
                Field::String(format!("key_{}", self.ctr))
                    .to_bytes()?
                    .as_slice(),
            )?;
        assert!(v.is_some());
        self.ctr += 1;

        fw.send(op, RECORD_READER_PROCESSOR_OUTPUT_PORT)
    }

    fn commit(&self, _tx: &mut dyn RwTransaction) -> Result<(), ExecutionError> {
        info!("Commit notification");
        Ok(())
    }
}

#[test]
fn test_run_dag_reacord_reader() {
    // log4rs::init_file("../config/log4rs.sample.yaml", Default::default())
    //     .unwrap_or_else(|_e| panic!("Unable to find log4rs config file"));

    let src = GeneratorSourceFactory::new(1_000);
    let passthrough = PassthroughProcessorFactory::new();
    let record_reader = RecordReaderProcessorFactory::new();
    let sink = CountingSinkFactory::new(1_000);

    let mut dag = Dag::new();

    let SOURCE_ID: NodeHandle = "source".to_string();
    let PASSTHROUGH_ID: NodeHandle = "passthrough".to_string();
    let RECORD_READER_ID: NodeHandle = "record_reader".to_string();
    let SINK_ID: NodeHandle = "sink".to_string();

    dag.add_node(NodeType::Source(Box::new(src)), SOURCE_ID.clone());
    dag.add_node(
        NodeType::Processor(Box::new(passthrough)),
        PASSTHROUGH_ID.clone(),
    );
    dag.add_node(
        NodeType::Processor(Box::new(record_reader)),
        RECORD_READER_ID.clone(),
    );
    dag.add_node(NodeType::Sink(Box::new(sink)), SINK_ID.clone());

    assert!(dag
        .connect(
            Endpoint::new(SOURCE_ID, GENERATOR_SOURCE_OUTPUT_PORT),
            Endpoint::new(PASSTHROUGH_ID.clone(), PASSTHROUGH_PROCESSOR_INPUT_PORT),
        )
        .is_ok());

    assert!(dag
        .connect(
            Endpoint::new(PASSTHROUGH_ID, PASSTHROUGH_PROCESSOR_OUTPUT_PORT),
            Endpoint::new(RECORD_READER_ID.clone(), RECORD_READER_PROCESSOR_INPUT_PORT),
        )
        .is_ok());

    assert!(dag
        .connect(
            Endpoint::new(RECORD_READER_ID, RECORD_READER_PROCESSOR_OUTPUT_PORT),
            Endpoint::new(SINK_ID, COUNTING_SINK_INPUT_PORT),
        )
        .is_ok());

    let tmp_dir = chk!(TempDir::new("example"));
    if tmp_dir.path().exists() {
        chk!(fs::remove_dir_all(tmp_dir.path()));
    }
    chk!(fs::create_dir(tmp_dir.path()));

    let exec = chk!(MultiThreadedDagExecutor::start(
        dag,
        tmp_dir.path(),
        ExecutorOptions::default()
    ));

    assert!(exec.join().is_ok());
}

#[test]
fn test_run_dag_reacord_reader_from_stateful_src() {
    // log4rs::init_file("../config/log4rs.sample.yaml", Default::default())
    //     .unwrap_or_else(|_e| panic!("Unable to find log4rs config file"));

    let src = StatefulGeneratorSourceFactory::new(1000, Duration::from_micros(0));
    let record_reader = RecordReaderProcessorFactory::new();
    let sink = CountingSinkFactory::new(1_000);

    let mut dag = Dag::new();

    let SOURCE_ID: NodeHandle = "source".to_string();
    let RECORD_READER_ID: NodeHandle = "record_reader".to_string();
    let SINK_ID: NodeHandle = "sink".to_string();

    dag.add_node(NodeType::Source(Box::new(src)), SOURCE_ID.clone());
    dag.add_node(
        NodeType::Processor(Box::new(record_reader)),
        RECORD_READER_ID.clone(),
    );
    dag.add_node(NodeType::Sink(Box::new(sink)), SINK_ID.clone());

    assert!(dag
        .connect(
            Endpoint::new(SOURCE_ID, GENERATOR_SOURCE_OUTPUT_PORT),
            Endpoint::new(RECORD_READER_ID.clone(), RECORD_READER_PROCESSOR_INPUT_PORT),
        )
        .is_ok());

    assert!(dag
        .connect(
            Endpoint::new(RECORD_READER_ID, RECORD_READER_PROCESSOR_OUTPUT_PORT),
            Endpoint::new(SINK_ID, COUNTING_SINK_INPUT_PORT),
        )
        .is_ok());

    let tmp_dir = chk!(TempDir::new("example"));
    if tmp_dir.path().exists() {
        chk!(fs::remove_dir_all(tmp_dir.path()));
    }
    chk!(fs::create_dir(tmp_dir.path()));

    let exec = chk!(MultiThreadedDagExecutor::start(
        dag,
        tmp_dir.path(),
        ExecutorOptions::default()
    ));

    assert!(exec.join().is_ok());
}

#[test]
fn test_run_dag_reacord_reader_from_stateful_src_timeout() {
    // log4rs::init_file("../config/log4rs.sample.yaml", Default::default())
    //     .unwrap_or_else(|_e| panic!("Unable to find log4rs config file"));

    let src = StatefulGeneratorSourceFactory::new(30, Duration::from_millis(200));
    let record_reader = RecordReaderProcessorFactory::new();
    let sink = CountingSinkFactory::new(1_000);

    let mut dag = Dag::new();

    let SOURCE_ID: NodeHandle = "source".to_string();
    let RECORD_READER_ID: NodeHandle = "record_reader".to_string();
    let SINK_ID: NodeHandle = "sink".to_string();

    dag.add_node(NodeType::Source(Box::new(src)), SOURCE_ID.clone());
    dag.add_node(
        NodeType::Processor(Box::new(record_reader)),
        RECORD_READER_ID.clone(),
    );
    dag.add_node(NodeType::Sink(Box::new(sink)), SINK_ID.clone());

    assert!(dag
        .connect(
            Endpoint::new(SOURCE_ID, GENERATOR_SOURCE_OUTPUT_PORT),
            Endpoint::new(RECORD_READER_ID.clone(), RECORD_READER_PROCESSOR_INPUT_PORT),
        )
        .is_ok());

    assert!(dag
        .connect(
            Endpoint::new(RECORD_READER_ID, RECORD_READER_PROCESSOR_OUTPUT_PORT),
            Endpoint::new(SINK_ID, COUNTING_SINK_INPUT_PORT),
        )
        .is_ok());

    let tmp_dir = chk!(TempDir::new("example"));
    if tmp_dir.path().exists() {
        chk!(fs::remove_dir_all(tmp_dir.path()));
    }
    chk!(fs::create_dir(tmp_dir.path()));

    let mut exec_opts = ExecutorOptions::default();
    exec_opts.commit_time_threshold = Duration::from_millis(2000);

    let exec = chk!(MultiThreadedDagExecutor::start(
        dag,
        tmp_dir.path(),
        exec_opts
    ));

    assert!(exec.join().is_ok());
}