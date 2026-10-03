# Style notes: ADSL (Arpaci-Dusseau) papers

SCL = Patel et al., EuroSys '20; ALICE = Pillai et al., OSDI '14; Split = Yang et al., SOSP '15;
CORDS = Ganesan et al., FAST '17. Quotes are verbatim.

- **Title.** Short: the idea's name ("Split-Level I/O Scheduling"), problem + fix ("Avoiding Scheduler
  Subversion using Scheduler-Cooperative Locks"), or a claim ("Redundancy Does Not Imply Fault Tolerance: ...").
- **Open broad, narrow fast.** "In the modern shared datacenter, scheduling of resources is of central
  importance." (SCL) Then: "Unfortunately, making decisions at the block level is problematic, for two
  reasons." (Split)
- **Name the problem in italics; define it in one sentence.** "We introduce the *scheduler subversion*
  problem, where lock usage patterns determine which thread runs, thereby subverting CPU scheduling goals." (SCL)
- **Tiny running example early.** "As a simple example, consider two processes, P0 and P1, where each thread
  spends a large fraction of time competing for lock L." (SCL)
- **State the question.** "In this paper, we address these two challenges directly, by answering two important
  questions." (ALICE)
- **"In this paper, we ..." then "We find that ..."** "In this paper, we introduce *split-level I/O
  scheduling*, a novel scheduling framework ..." (Split) "We find that persistence properties vary widely
  among the tested file systems." (ALICE)
- **Counted contributions, each with evidence.** "This paper contains three major contributions. First, we
  build a fault injection framework (CORDS) ..." (CORDS)
- **Roadmap.** "The rest of this paper is organized as follows. We first discuss the scheduler subversion
  problem in Section 2 ..." (SCL)
- **Section openers say what the section does; closers summarise.** "In this section, we describe how existing
  locks do not guarantee lock usage fairness ..." (SCL) "**Summary:** Locks can subvert scheduling goals
  depending on the workload and how locks are accessed." (SCL)
- **Motivation demonstrates with one figure.** "We demonstrate this problem by running a normal process A
  alongside an idle-priority process B." (Split)
- **Goals, then one mechanism per need.** "Our SCL design is guided by four high-level goals:" with bold goal
  names; the design then has bold components ("Lock usage accounting.", "... lock slice.").
- **Evaluation subsections open with purpose, close with a verdict.** "Minimal overhead was one of our goals
  while designing u-SCL." ... "In summary, we show that u-SCL provides lock opportunity to all threads ..." (SCL)
- **Explicit limitations section.** "In this section, we discuss the limitations and applicability of SCLs." (SCL)
- **Conclusion: restate, then one hope.** "In this paper, we have demonstrated that locks can subvert scheduling
  goals ..." (SCL) "Our hope is that split-level scheduling will inspire future vertical integration in
  storage stacks." (Split)
- **Voice.** First-person plural, plain verbs, short sentences, few adjectives, bold run-in paragraph heads.
