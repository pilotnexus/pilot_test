
extern crate pilot_macro;
extern crate pilot_sys;

pub use pilot_macro::*;

use pilot_sys::async_util::raw_waker;
use pilot_sys::time::set_system_time;
use pilot_sys::async_util::State;
use core::{
    task::{Context, Waker, Poll},
};
use futures::future::{Future, FutureExt};

use std::fmt::{Debug};
use std::cmp::Ordering;
use std::convert::TryInto;

use core::time::Duration;

#[derive(Debug)]
#[derive(PartialEq)]
pub enum TestFutureResult<T> where T: PartialEq {
    Output(T),
    Done
}

fn test_runner<T: PartialEq>(f: impl Future<Output=T>, run_to: Duration, step: Duration, execute: &mut Vec<TestExecutor>, rules: &mut Vec<RuleExecutor>) -> (Duration, TestFutureResult<T>) {
    let future = f.fuse();
    futures::pin_mut!(future);
    let mut state = State { future };

    let run_to_us: i64 = run_to.num_microseconds().unwrap_or(i64::MAX);
    let step: usize = step.num_microseconds().unwrap().try_into().unwrap();

    for us in (0..run_to_us).step_by(step) {
        let us_duration = Duration::microseconds(us);
        // Program Loop
        set_system_time(us.try_into().unwrap());
        
        //run rules
        for ref mut rule in rules.iter_mut() {
            rule.run(us_duration);
        }

        //prepare async executor
        let waker = unsafe { Waker::from_raw(raw_waker()) };
        let mut context = Context::from_waker(&waker);

        match state.future.as_mut().poll(&mut context) {
            Poll::Ready(result) => return (us_duration, TestFutureResult::Output(result)),
            _ => {} //not ready, continue
        };

        //update stuff
        for exec in execute.iter_mut() {
            let mut done = true;
            while done {
                if exec.current_step < exec.action.len() {
                    let mut item = exec
                        .action
                        .get_mut(exec.current_step)
                        .expect("cannot get action");

                    if item.run_at <= us_duration {
                        //check if it is time to run item
                        if !item.done {
                            let can_progress = (item.run)(us_duration);
                            //let can_progress = match item.run {
                            //  Func::Run(f) => { 
                            //    f();
                            //    true
                            //  },
                            //  Func::RunT(f) => { 
                            //    f(us);
                            //    true
                            //  },
                            //  Func::IsTrue(f) => f() == true,
                            //  Func::IsFalse(f) => f() == false,
                            //};

                            if can_progress {
                              item.done = true;
                              done = item.done;
                              exec.current_step += 1;
                            } else {
                              break;
                            }
                        }
                    } else {
                        break; //not yet time
                    }
                } else {
                    break; // all items are processed
                }
            }
        }
    }
    (run_to, TestFutureResult::Done)
}

pub enum Time {
    Abs(Duration),
    After(Duration),
}

//pub enum Func {
//  Run(fn() -> ()),
//  RunT(fn(u64) -> ()),
//  IsTrue(fn() -> bool),
//  IsFalse(fn() -> bool),
//}

struct Step {
    pub run_at: Duration,
    pub run: Box<dyn FnMut(Duration) -> bool>,
    pub done: bool,
}

impl Ord for Step {
    fn cmp(&self, other: &Self) -> Ordering {
        self.run_at.cmp(&other.run_at)
    }
}

impl PartialOrd for Step {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Step {
    fn eq(&self, other: &Self) -> bool {
        self.run_at == other.run_at
    }
}

impl Eq for Step {}


trait Rule {
    //type Predicate;
    fn predicate(&mut self, us: Duration) -> bool;
    /// reset rule state
    fn reset(&mut self);
}

struct PredicateTrueRule {
    can_progress: Box::<dyn FnMut() -> bool>,
}

impl Rule for PredicateTrueRule {
    fn predicate(&mut self, _us: Duration) -> bool {
        (self.can_progress)()
    }

    fn reset(&mut self) {}
}

struct WaitRule {
    duration: Duration,
    wait_until: Option<Duration>
}

impl Rule for WaitRule {
    fn predicate(&mut self, us: Duration) -> bool {

        if self.wait_until == None {
            self.wait_until = Some(us+self.duration)
        }

        match self.wait_until {
            None => unreachable!(),
            Some(v) => us>= v
        }
    }

    fn reset(&mut self) {
        self.wait_until = None
    }
}

struct ThenRule {
    progress: Box::<dyn FnMut()>,
}

impl Rule for ThenRule {
    fn predicate(&mut self, _us: Duration) -> bool {
        (self.progress)();
        true
    }

    fn reset(&mut self) {}
}


pub struct RuleBuilder {
    rules: Vec<Box<dyn Rule>>
}

impl RuleBuilder {
    pub fn new() -> Self {
        Self {
            rules: Vec::<Box<dyn Rule>>::new()
        }
    }

    pub fn if_true<F>(mut self, func: F) -> RuleBuilder
        where F: FnMut() -> bool + 'static
    {
        self.rules.push( Box::new(PredicateTrueRule {
            can_progress: Box::new(func)
        }));

        self
    }

    pub fn wait(mut self, wait_for_us: Duration) -> RuleBuilder {
        self.rules.push( Box::new(WaitRule {
            duration: wait_for_us,
            wait_until: None
        }));

        self
    }

    pub fn then<F>(mut self, func: F) -> RuleBuilder
        where F: FnMut() + 'static
    {
        self.rules.push( Box::new(ThenRule {
            progress: Box::new(func)
        }));

        self
    }

    pub fn build(self) -> RuleExecutor {
        RuleExecutor {
            current_rule_index: 0,
            rules: self.rules
        }
    }
}

pub struct RuleExecutor {
   current_rule_index: usize,
   rules: Vec<Box<dyn Rule>>
}

impl RuleExecutor {
    pub fn run(&mut self, us: Duration) {
        if self.rules[self.current_rule_index].predicate(us) {
            if self.current_rule_index == self.rules.len() - 1 {
                //restart ruleset, reset behaviour
                for rule in self.rules.iter_mut() {
                    rule.reset();
                }
                self.current_rule_index = 0;
            } else {
                self.current_rule_index += 1;
            }
            eprintln!("Rule progressed to {} at {}", self.current_rule_index, us );
        }
    }
}


pub struct TestExecutor {
    current_step: usize,
    step_size: Duration,
    action: Vec<Step>,
}

impl TestExecutor {
    pub fn new() -> Self {
        Self {
            current_step: 0,
            step_size: Duration::milliseconds(1),
            action: vec![],
        }
    }

    pub fn run_limit<T: PartialEq>(self, f: impl Future<Output=T>, run_to: Duration, rules: &mut Vec<RuleExecutor>) -> (Duration, TestFutureResult<T>) {
        let mut v = Vec::new();
        let step = self.step_size;
        v.push(self);
        test_runner(f, run_to, step, &mut v, rules)
    }

    pub fn run<T: PartialEq>(self, f: impl Future<Output=T>, rules: &mut Vec<RuleExecutor>) -> (Duration, TestFutureResult<T>) {
        self.run_limit(f, Duration::max_value(), rules)
    }
}

/// TestBuilder uses the builder-pattern to generate a list of [`TestExecutor`] objects which in turn can be used to run a test.
/// Examples:
/// ''' rust
/// use pilot_test::{ TestBuilder, TestFutureResult, Time, Func };
/// use super::{VARS};
///
/// let result = TestBuilder::new()
/// .at(Time::Abs(0), Func::Run(|| {}))
/// .build()
/// .run(super::cyl(&VARS.in0, &VARS.out0));
///
/// assert_eq!(result, TestFutureResult::Output(Result::Ok(())));
/// '''
pub struct TestBuilder {
    last_time: Duration,
    executor: TestExecutor,
}

impl TestBuilder {
    pub fn new() -> Self {
        Self {
            last_time: Duration::zero(),
            executor: TestExecutor::new(),
        }
    }

    fn get_time(&self, time: Time) -> Duration {
        match time {
            Time::Abs(t) => t,
            Time::After(t) => self.last_time + t,
        }
    }

    pub fn at<F>(mut self, time: Time, run: F) -> TestBuilder
        where F: FnMut(Duration) -> bool,
        F: 'static {
        let run_at = self.get_time(time);
        self.executor.action.push(Step {
            run_at,
            run: Box::new(run),
            done: false,
        });
        self.last_time = run_at;

        self
    }

    //pub fn at<F>(mut self, time: Time, run: F) -> TestBuilder
    //    where F: FnMut(u64),
    //    F: 'static {
    //    let run_at = self.get_time(time);
    //    self.executor.action.push(Step {
    //        run_at,
    //        run: Box::new(move |us| { run(us); true }),
    //        done: false,
    //    });
    //    self.last_time = run_at;

    //    self
    //}

    //pub fn pulse<T: Default>(mut self, time: Time, pulse_length: u64, value: &'static Var<T>, pulse_value: T) -> TestBuilder
    //    where Var<T>: VarProps<T>, T: Copy {
    //    let new_value = pulse_value;
    //    let old_value = value.get();
    //    let mut run_at = self.get_time(time);

    //    //send pulse on
    //    self.executor.action.push(Step {
    //        run_at,
    //        run: Box::new(|us| { 
    //          (*value).set(*pulse_value); 
    //          true
    //        }),
    //        done: false,
    //    });

    //    run_at += pulse_length;

    //    //send pulse off
    //    self.executor.action.push(Step {
    //        run_at,
    //        run: Box::new(|us| { 
    //          (*value).set(old_value); 
    //          true
    //          }),
    //        done: false,
    //    });

    //    self.last_time = run_at;
    //    
    //    self
    //}

    pub fn build(mut self) -> TestExecutor {
        let ref mut action = self.executor.action;
        action.sort();
        self.executor
    }
}
