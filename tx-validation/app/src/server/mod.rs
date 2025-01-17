use crate::enclave_u::{check_deposit_tx, check_initchain, check_transfertx, check_withdraw_tx};
use chain_core::state::account::DepositBondTx;
use chain_core::tx::data::input::TxoPointer;
use chain_core::tx::TxAux;
use enclave_protocol::{EnclaveRequest, EnclaveResponse, FLAGS};
use log::{debug, info};
use parity_scale_codec::{Decode, Encode};
use sgx_urts::SgxEnclave;
use sled::Tree;
use std::sync::Arc;
use zmq::{Context, Error, Socket, REP};

pub struct TxValidationServer {
    socket: Socket,
    enclave: SgxEnclave,
    txdb: Arc<Tree>,
}

impl TxValidationServer {
    pub fn new(
        connection_str: &str,
        enclave: SgxEnclave,
        txdb: Arc<Tree>,
    ) -> Result<TxValidationServer, Error> {
        let ctx = Context::new();
        let socket = ctx.socket(REP)?;
        socket.bind(connection_str)?;
        Ok(TxValidationServer {
            socket,
            enclave,
            txdb,
        })
    }

    fn lookup_txids(&self, inputs: &[TxoPointer]) -> Option<Vec<Vec<u8>>> {
        let mut result = Vec::with_capacity(inputs.len());
        for input in inputs.iter() {
            if let Ok(Some(txin)) = self.txdb.get(input.id) {
                result.push(txin.to_vec());
            } else {
                return None;
            }
        }
        Some(result)
    }

    fn lookup(&self, tx: &TxAux) -> Option<Vec<Vec<u8>>> {
        match tx {
            TxAux::TransferTx { inputs, .. } => self.lookup_txids(inputs),
            TxAux::DepositStakeTx {
                tx: DepositBondTx { inputs, .. },
                ..
            } => self.lookup_txids(inputs),
            _ => None,
        }
    }

    pub fn execute(&mut self) {
        info!("running zmq server");
        loop {
            if let Ok(msg) = self.socket.recv_bytes(FLAGS) {
                debug!("received a message");
                let mcmd = EnclaveRequest::decode(&mut msg.as_slice());
                let resp = match mcmd {
                    Ok(EnclaveRequest::CheckChain { chain_hex_id }) => {
                        debug!("check chain");
                        EnclaveResponse::CheckChain(check_initchain(
                            self.enclave.geteid(),
                            chain_hex_id,
                        ))
                    }
                    Ok(EnclaveRequest::VerifyTx {
                        tx: tx @ TxAux::TransferTx { .. },
                        info,
                        ..
                    }) => {
                        debug!("verify transfer tx");
                        if let Some(txins) = self.lookup(&tx) {
                            EnclaveResponse::VerifyTx(check_transfertx(
                                self.enclave.geteid(),
                                tx,
                                txins,
                                info,
                                self.txdb.clone(),
                            ))
                        } else {
                            EnclaveResponse::VerifyTx(Err(()))
                        }
                    }
                    Ok(EnclaveRequest::VerifyTx {
                        tx: tx @ TxAux::DepositStakeTx { .. },
                        info,
                        account,
                    }) => {
                        debug!("verify deposit tx");
                        if let Some(txins) = self.lookup(&tx) {
                            EnclaveResponse::VerifyTx(check_deposit_tx(
                                self.enclave.geteid(),
                                tx,
                                txins,
                                account,
                                info,
                                self.txdb.clone(),
                            ))
                        } else {
                            EnclaveResponse::VerifyTx(Err(()))
                        }
                    }
                    Ok(EnclaveRequest::VerifyTx {
                        tx: tx @ TxAux::WithdrawUnbondedStakeTx { .. },
                        info,
                        account: Some(account),
                        ..
                    }) => {
                        debug!("verify withdraw tx");
                        EnclaveResponse::VerifyTx(check_withdraw_tx(
                            self.enclave.geteid(),
                            tx,
                            account,
                            info,
                            self.txdb.clone(),
                        ))
                    }
                    Ok(_) => {
                        debug!("verify other tx");
                        EnclaveResponse::UnsupportedTxType
                    }
                    Err(e) => {
                        debug!("unknown request / failed to decode: {}", e);
                        EnclaveResponse::UnsupportedTxType
                    }
                };
                let response = resp.encode();
                self.socket
                    .send(response, FLAGS)
                    .expect("reply sending failed");
            }
        }
    }
}
