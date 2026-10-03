use pipecircuit::{Circuit, CircuitReactionOutcome, Reactor, Switchboard};

pub(crate) struct BoundedCircuit<C> {
    pub(crate) circuit: C,
    remaining: usize,
}

impl<C> BoundedCircuit<C> {
    pub(crate) fn new(circuit: C, reactions: usize) -> Self {
        Self {
            circuit,
            remaining: reactions,
        }
    }
}

impl<B: Switchboard, C: Circuit<B>> Circuit<B> for BoundedCircuit<C> {
    type Reactor = C::Reactor;
    type Completion = C::Completion;
    type Failure = C::Failure;

    fn reactor(&mut self) -> &mut Self::Reactor {
        self.circuit.reactor()
    }

    fn react(
        &mut self,
        board: &mut B,
        reaction: &<Self::Reactor as Reactor>::Reaction,
    ) -> Result<CircuitReactionOutcome<Self::Completion>, Self::Failure> {
        self.remaining = self
            .remaining
            .checked_sub(1)
            .expect("circuit exceeded its scripted reaction budget");
        self.circuit.react(board, reaction)
    }
}
