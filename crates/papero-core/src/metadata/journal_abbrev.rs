use crate::metadata::models::BibliographicRecord;
use crate::text_norm::normalize_math_text;
use std::collections::HashSet;

const DOTLESS_CANONICAL_JOURNALS: &[&str] = &[
    "algebra number theory",
    "asterisque",
    "bernoulli",
    "biometrika",
    "bit",
    "chaos solitons fractals",
    "combinatorica",
    "econometrica",
    "erkenntnis",
    "ergodic theory dynam systems",
    "experimental mathematics",
    "inverse problems",
    "k theory",
    "mathematika",
    "mind",
    "nature",
    "nonlinearity",
    "nous",
    "psychometrika",
    "random structures algorithms",
    "representation theory",
    "science",
    "synthese",
    "technometrics",
    "topology",
];

// Curated canonical AMS / Mathematical Reviews / zbMATH journal abbreviations
// indexed by normalized (`normalize_math_text`) full title.
const CURATED_JOURNAL_ABBREVS: &[(&str, &str)] = &[
    ("abstract and applied analysis", "Abstr. Appl. Anal."),
    ("acta arithmetica", "Acta Arith."),
    ("acta mathematica", "Acta Math."),
    ("acta scientiarum mathematicarum", "Acta Sci. Math. (Szeged)"),
    ("advances in applied mathematics", "Adv. in Appl. Math."),
    ("advances in differential equations", "Adv. Differential Equations"),
    ("advances in geometry", "Adv. Geom."),
    ("advances in mathematics", "Adv. Math."),
    ("algebra and logic", "Algebra Logic"),
    ("algebra number theory", "Algebra Number Theory"),
    ("algebra universalis", "Algebr. Universalis"),
    ("algebraic geometric topology", "Algebr. Geom. Topol."),
    ("algebraic geometry", "Algebr. Geom."),
    ("american journal of mathematics", "Amer. J. Math."),
    ("american mathematical monthly", "Amer. Math. Monthly"),
    ("the american mathematical monthly", "Amer. Math. Monthly"),
    ("annalen der physik", "Ann. Phys."),
    ("annales de l institut fourier", "Ann. Inst. Fourier"),
    (
        "annales de l institut henri poincare analyse non lineaire",
        "Ann. Inst. H. Poincaré Anal. Non Linéaire",
    ),
    (
        "annales de l institut henri poincare c analyse non lineaire",
        "Ann. Inst. H. Poincaré C Anal. Non Linéaire",
    ),
    (
        "annales de l institut henri poincare probabilites et statistiques",
        "Ann. Inst. Henri Poincaré Probab. Stat.",
    ),
    ("annales henri poincare", "Ann. Henri Poincaré"),
    ("annales polonici mathematici", "Ann. Polon. Math."),
    (
        "annales scientifiques de l ecole normale superieure",
        "Ann. Sci. Éc. Norm. Supér.",
    ),
    ("annali della scuola normale superiore di pisa", "Ann. Sc. Norm. Super. Pisa Cl. Sci."),
    (
        "annali della scuola normale superiore di pisa classe di scienze",
        "Ann. Sc. Norm. Super. Pisa Cl. Sci.",
    ),
    ("annali di matematica pura ed applicata", "Ann. Mat. Pura Appl."),
    ("annals of applied probability", "Ann. Appl. Probab."),
    ("the annals of applied probability", "Ann. Appl. Probab."),
    ("annals of global analysis and geometry", "Ann. Global Anal. Geom."),
    ("annals of mathematics", "Ann. of Math."),
    ("annals of mathematics second series", "Ann. of Math. (2)"),
    ("annals of mathematics and artificial intelligence", "Ann. Math. Artif. Intell."),
    ("annals of physics", "Ann. Physics"),
    ("annals of probability", "Ann. Probab."),
    ("the annals of probability", "Ann. Probab."),
    ("annals of pure and applied logic", "Ann. Pure Appl. Logic"),
    ("annals of statistics", "Ann. Statist."),
    ("the annals of statistics", "Ann. Statist."),
    ("applicable analysis", "Appl. Anal."),
    ("applied and computational harmonic analysis", "Appl. Comput. Harmon. Anal."),
    ("applied mathematics and computation", "Appl. Math. Comput."),
    ("applied mathematics and optimization", "Appl. Math. Optim."),
    ("applied mathematics letters", "Appl. Math. Lett."),
    ("archiv der mathematik", "Arch. Math."),
    ("archive for rational mechanics and analysis", "Arch. Ration. Mech. Anal."),
    ("arkiv for matematik", "Ark. Mat."),
    ("asymptotic analysis", "Asymptot. Anal."),
    ("biometrika", "Biometrika"),
    ("bit numerical mathematics", "BIT"),
    ("bollettino dell unione matematica italiana", "Boll. Unione Mat. Ital."),
    ("boundary value problems", "Boundary Value Probl."),
    (
        "bulletin de la societe mathematique de france",
        "Bull. Soc. Math. Fr.",
    ),
    ("bulletin des sciences mathematiques", "Bull. Sci. Math."),
    ("bulletin of mathematical sciences", "Bull. Math. Sci."),
    (
        "bulletin of the american mathematical society",
        "Bull. Amer. Math. Soc.",
    ),
    (
        "bulletin of the australian mathematical society",
        "Bull. Aust. Math. Soc.",
    ),
    (
        "bulletin of the london mathematical society",
        "Bull. Lond. Math. Soc.",
    ),
    (
        "calculus of variations and partial differential equations",
        "Calc. Var. Partial Differential Equations",
    ),
    ("cambridge journal of mathematics", "Camb. J. Math."),
    ("canadian journal of mathematics", "Canad. J. Math."),
    ("canadian mathematical bulletin", "Canad. Math. Bull."),
    ("chaos solitons fractals", "Chaos Solitons Fractals"),
    ("collectanea mathematica", "Collect. Math."),
    ("colloquium mathematicum", "Colloq. Math."),
    ("combinatorics probability and computing", "Combin. Probab. Comput."),
    ("commentarii mathematici helvetici", "Comment. Math. Helv."),
    (
        "commentationes mathematicae universitatis carolinae",
        "Comment. Math. Univ. Carolin.",
    ),
    ("communications in algebra", "Comm. Algebra"),
    ("communications in analysis and geometry", "Comm. Anal. Geom."),
    ("communications in contemporary mathematics", "Commun. Contemp. Math."),
    ("communications in mathematical physics", "Comm. Math. Phys."),
    (
        "communications in partial differential equations",
        "Comm. Partial Differential Equations",
    ),
    (
        "communications on pure and applied analysis",
        "Commun. Pure Appl. Anal.",
    ),
    (
        "communications on pure and applied mathematics",
        "Comm. Pure Appl. Math.",
    ),
    ("compositio mathematica", "Compos. Math."),
    ("computational complexity", "Comput. Complexity"),
    ("computational geometry", "Comput. Geom."),
    (
        "computer methods in applied mechanics and engineering",
        "Comput. Methods Appl. Mech. Engrg.",
    ),
    ("constructive approximation", "Constr. Approx."),
    ("comptes rendus de l academie des sciences", "C. R. Acad. Sci. Paris"),
    ("comptes rendus mathematique", "C. R. Math."),
    ("differential and integral equations", "Differential Integral Equations"),
    ("differential geometry and its applications", "Differential Geom. Appl."),
    ("discrete computational geometry", "Discrete Comput. Geom."),
    ("discrete and continuous dynamical systems", "Discrete Contin. Dyn. Syst."),
    ("discrete mathematics", "Discrete Math."),
    ("documenta mathematica", "Doc. Math."),
    ("duke mathematical journal", "Duke Math. J."),
    ("electronic communications in probability", "Electron. Commun. Probab."),
    ("electronic journal of combinatorics", "Electron. J. Combin."),
    ("the electronic journal of combinatorics", "Electron. J. Combin."),
    ("electronic journal of differential equations", "Electron. J. Differential Equations"),
    ("electronic journal of probability", "Electron. J. Probab."),
    ("epijournal de geometrie algebrique", "Épijournal Géom. Algébrique"),
    ("ergodic theory and dynamical systems", "Ergodic Theory Dynam. Systems"),
    (
        "esaim control optimisation and calculus of variations",
        "ESAIM Control Optim. Calc. Var.",
    ),
    (
        "esaim mathematical modelling and numerical analysis",
        "ESAIM Math. Model. Numer. Anal.",
    ),
    ("european journal of combinatorics", "European J. Combin."),
    ("experimental mathematics", "Experiment. Math."),
    ("exposiones mathematicae", "Expo. Math."),
    ("finite fields and their applications", "Finite Fields Appl."),
    ("forum mathematicum", "Forum Math."),
    ("forum of mathematics pi", "Forum Math. Pi"),
    ("forum of mathematics sigma", "Forum Math. Sigma"),
    ("foundations of computational mathematics", "Found. Comput. Math."),
    ("functional analysis and its applications", "Funct. Anal. Appl."),
    ("fundamenta mathematicae", "Fund. Math."),
    ("geometriae dedicata", "Geom. Dedicata"),
    ("geometric and functional analysis", "Geom. Funct. Anal."),
    ("geometry topology", "Geom. Topol."),
    ("glasgow mathematical journal", "Glasg. Math. J."),
    ("groups geometry and dynamics", "Groups Geom. Dyn."),
    ("hokkaido mathematical journal", "Hokkaido Math. J."),
    (
        "ieee transactions on automatic control",
        "IEEE Trans. Automat. Control",
    ),
    (
        "ieee transactions on information theory",
        "IEEE Trans. Inform. Theory",
    ),
    (
        "ieee transactions on pattern analysis and machine intelligence",
        "IEEE Trans. Pattern Anal. Mach. Intell.",
    ),
    (
        "ieee transactions on signal processing",
        "IEEE Trans. Signal Process.",
    ),
    ("illinois journal of mathematics", "Illinois J. Math."),
    ("ima journal of numerical analysis", "IMA J. Numer. Anal."),
    ("indagationes mathematicae", "Indag. Math."),
    ("indiana university mathematics journal", "Indiana Univ. Math. J."),
    ("information and computation", "Inform. and Comput."),
    ("international journal of mathematics", "Internat. J. Math."),
    ("international journal of number theory", "Int. J. Number Theory"),
    (
        "international mathematics research notices",
        "Int. Math. Res. Not.",
    ),
    ("inventiones mathematicae", "Invent. Math."),
    ("inverse problems", "Inverse Problems"),
    ("israel journal of mathematics", "Israel J. Math."),
    ("izvestiya mathematics", "Izv. Math."),
    ("japanese journal of mathematics", "Jpn. J. Math."),
    (
        "journal de mathematiques pures et appliquees",
        "J. Math. Pures Appl.",
    ),
    (
        "journal de theorie des nombres de bordeaux",
        "J. Théor. Nombres Bordeaux",
    ),
    (
        "journal fur die reine und angewandte mathematik",
        "J. Reine Angew. Math.",
    ),
    ("journal of algebra", "J. Algebra"),
    (
        "journal of algebra and its applications",
        "J. Algebra Appl.",
    ),
    ("journal of algebraic combinatorics", "J. Algebraic Combin."),
    ("journal of algebraic geometry", "J. Algebraic Geom."),
    ("journal of analysis and its applications", "J. Anal. Appl."),
    ("journal of approximation theory", "J. Approx. Theory"),
    ("journal of combinatorial designs", "J. Combin. Des."),
    (
        "journal of combinatorial theory series a",
        "J. Combin. Theory Ser. A",
    ),
    (
        "journal of combinatorial theory series b",
        "J. Combin. Theory Ser. B",
    ),
    ("journal of complexity", "J. Complexity"),
    (
        "journal of computational and applied mathematics",
        "J. Comput. Appl. Math.",
    ),
    ("journal of computational physics", "J. Comput. Phys."),
    ("journal of computer and system sciences", "J. Comput. System Sci."),
    ("journal of convex analysis", "J. Convex Anal."),
    ("journal of cryptology", "J. Cryptology"),
    ("journal of differential equations", "J. Differential Equations"),
    ("journal of differential geometry", "J. Differential Geom."),
    ("journal of dynamics and differential equations", "J. Dynam. Differential Equations"),
    ("journal of evolution equations", "J. Evol. Equ."),
    ("journal of fourier analysis and applications", "J. Fourier Anal. Appl."),
    ("journal of functional analysis", "J. Funct. Anal."),
    ("journal of geometric analysis", "J. Geom. Anal."),
    ("the journal of geometric analysis", "J. Geom. Anal."),
    ("journal of geometric mechanics", "J. Geom. Mech."),
    ("journal of geometry and physics", "J. Geom. Phys."),
    ("journal of graph theory", "J. Graph Theory"),
    ("journal of group theory", "J. Group Theory"),
    ("journal of hyperbolic differential equations", "J. Hyperbolic Differ. Equ."),
    (
        "journal of knot theory and its ramifications",
        "J. Knot Theory Ramifications",
    ),
    ("journal of lie theory", "J. Lie Theory"),
    ("journal of machine learning research", "J. Mach. Learn. Res."),
    (
        "journal of mathematical analysis and applications",
        "J. Math. Anal. Appl.",
    ),
    ("journal of mathematical imaging and vision", "J. Math. Imaging Vis."),
    ("journal of mathematical logic", "J. Math. Log."),
    ("journal of mathematical physics", "J. Math. Phys."),
    ("journal of mathematical sciences", "J. Math. Sci."),
    ("journal of modern dynamics", "J. Mod. Dyn."),
    ("journal of nonlinear science", "J. Nonlinear Sci."),
    ("journal of number theory", "J. Number Theory"),
    ("journal of operator theory", "J. Operator Theory"),
    ("journal of optimization theory and applications", "J. Optim. Theory Appl."),
    ("journal of pure and applied algebra", "J. Pure Appl. Algebra"),
    ("journal of scientific computing", "J. Sci. Comput."),
    ("journal of spectral theory", "J. Spectr. Theory"),
    ("journal of statistical physics", "J. Statist. Phys."),
    ("journal of symbolic computation", "J. Symbolic Comput."),
    ("journal of symbolic logic", "J. Symbolic Logic"),
    ("the journal of symbolic logic", "J. Symbolic Logic"),
    ("journal of symplectic geometry", "J. Symplectic Geom."),
    ("journal of the acm", "J. ACM"),
    (
        "journal of the american mathematical society",
        "J. Amer. Math. Soc.",
    ),
    (
        "journal of the australian mathematical society",
        "J. Aust. Math. Soc.",
    ),
    (
        "journal of the european mathematical society",
        "J. Eur. Math. Soc.",
    ),
    (
        "journal of the institute of mathematics of jussieu",
        "J. Inst. Math. Jussieu",
    ),
    (
        "journal of the london mathematical society",
        "J. Lond. Math. Soc.",
    ),
    (
        "journal of the mathematical society of japan",
        "J. Math. Soc. Japan",
    ),
    ("journal of topology", "J. Topol."),
    ("journal of topology and analysis", "J. Topol. Anal."),
    ("kobe journal of mathematics", "Kobe J. Math."),
    ("kyoto journal of mathematics", "Kyoto J. Math."),
    ("l enseignement mathematique", "Enseign. Math."),
    ("letters in mathematical physics", "Lett. Math. Phys."),
    ("linear algebra and its applications", "Linear Algebra Appl."),
    ("manuscripta mathematica", "Manuscripta Math."),
    ("mathematica scandinavica", "Math. Scand."),
    ("mathematical methods in the applied sciences", "Math. Methods Appl. Sci."),
    ("mathematical models and methods in applied sciences", "Math. Models Methods Appl. Sci."),
    ("mathematical notes", "Math. Notes"),
    (
        "mathematical proceedings of the cambridge philosophical society",
        "Math. Proc. Cambridge Philos. Soc.",
    ),
    ("mathematical programming", "Math. Program."),
    ("mathematical research letters", "Math. Res. Lett."),
    ("mathematical tables and other aids to computation", "Math. Tables Aids Comput."),
    ("mathematics of computation", "Math. Comp."),
    ("mathematische annalen", "Math. Ann."),
    ("mathematische nachrichten", "Math. Nachr."),
    ("mathematische zeitschrift", "Math. Z."),
    (
        "memoires de la societe mathematique de france",
        "Mém. Soc. Math. Fr.",
    ),
    (
        "memoirs of the american mathematical society",
        "Mem. Amer. Math. Soc.",
    ),
    ("michigan mathematical journal", "Michigan Math. J."),
    ("monatshefte fur mathematik", "Monatsh. Math."),
    ("nagoya mathematical journal", "Nagoya Math. J."),
    ("noesis", "Noesis"),
    ("nonlinear analysis", "Nonlinear Anal."),
    ("nonlinear differential equations and applications", "NoDEA Nonlinear Differential Equations Appl."),
    ("nonlinearity", "Nonlinearity"),
    (
        "notices of the american mathematical society",
        "Notices Amer. Math. Soc.",
    ),
    ("numerische mathematik", "Numer. Math."),
    ("osaka journal of mathematics", "Osaka J. Math."),
    ("pacific journal of mathematics", "Pacific J. Math."),
    ("pacific journal of optimization", "Pac. J. Optim."),
    ("peking mathematical journal", "Peking Math. J."),
    ("physical review d", "Phys. Rev. D"),
    ("physical review e", "Phys. Rev. E"),
    ("physical review letters", "Phys. Rev. Lett."),
    ("potential analysis", "Potent. Anal."),
    ("probability theory and related fields", "Probab. Theory Related Fields"),
    (
        "proceedings of the american mathematical society",
        "Proc. Amer. Math. Soc.",
    ),
    (
        "proceedings of the edinburgh mathematical society",
        "Proc. Edinb. Math. Soc.",
    ),
    (
        "proceedings of the london mathematical society",
        "Proc. Lond. Math. Soc.",
    ),
    (
        "proceedings of the national academy of sciences",
        "Proc. Natl. Acad. Sci. USA",
    ),
    (
        "proceedings of the national academy of sciences of the united states of america",
        "Proc. Natl. Acad. Sci. USA",
    ),
    (
        "proceedings of the steklov institute of mathematics",
        "Proc. Steklov Inst. Math.",
    ),
    ("publicacions matematiques", "Publ. Mat."),
    (
        "publications mathematiques de l ihes",
        "Publ. Math. Inst. Hautes Études Sci.",
    ),
    (
        "publications mathematiques de l institut des hautes etudes scientifiques",
        "Publ. Math. Inst. Hautes Études Sci.",
    ),
    (
        "publications mathematiques institut des hautes etudes scientifiques",
        "Publ. Math. Inst. Hautes Études Sci.",
    ),
    ("pure and applied mathematics quarterly", "Pure Appl. Math. Q."),
    ("quarterly journal of mathematics", "Q. J. Math."),
    ("the quarterly journal of mathematics", "Q. J. Math."),
    ("quarterly of applied mathematics", "Quart. Appl. Math."),
    ("ramanujan journal", "Ramanujan J."),
    ("the ramanujan journal", "Ramanujan J."),
    ("random structures algorithms", "Random Structures Algorithms"),
    (
        "rendiconti del seminario matematico della universita di padova",
        "Rend. Semin. Mat. Univ. Padova",
    ),
    ("representation theory", "Represent. Theory"),
    ("revista matematica iberoamericana", "Rev. Mat. Iberoam."),
    ("reviews in mathematical physics", "Rev. Math. Phys."),
    ("reviews of modern physics", "Rev. Modern Phys."),
    ("russian mathematical surveys", "Russian Math. Surveys"),
    ("sbornik mathematics", "Sb. Math."),
    ("selecta mathematica", "Selecta Math."),
    ("selecta mathematica new series", "Selecta Math. (N.S.)"),
    ("semigroup forum", "Semigroup Forum"),
    ("set valued and variational analysis", "Set-Valued Var. Anal."),
    ("siam journal on applied mathematics", "SIAM J. Appl. Math."),
    ("siam journal on computing", "SIAM J. Comput."),
    ("siam journal on control and optimization", "SIAM J. Control Optim."),
    ("siam journal on discrete mathematics", "SIAM J. Discrete Math."),
    ("siam journal on imaging sciences", "SIAM J. Imaging Sci."),
    ("siam journal on mathematical analysis", "SIAM J. Math. Anal."),
    ("siam journal on matrix analysis and applications", "SIAM J. Matrix Anal. Appl."),
    ("siam journal on numerical analysis", "SIAM J. Numer. Anal."),
    ("siam journal on optimization", "SIAM J. Optim."),
    ("siam journal on scientific computing", "SIAM J. Sci. Comput."),
    ("siam review", "SIAM Rev."),
    ("siberian mathematical journal", "Sib. Math. J."),
    ("stochastic processes and their applications", "Stochastic Process. Appl."),
    ("stochastics and dynamics", "Stoch. Dyn."),
    ("studia mathematica", "Studia Math."),
    ("theoretical computer science", "Theoret. Comput. Sci."),
    ("theory of computing", "Theory Comput."),
    ("tohoku mathematical journal", "Tohoku Math. J."),
    ("tohoku mathematical journal second series", "Tohoku Math. J. (2)"),
    ("topology", "Topology"),
    ("topology and its applications", "Topology Appl."),
    (
        "transactions of the american mathematical society",
        "Trans. Amer. Math. Soc.",
    ),
    ("transformation groups", "Transform. Groups"),
    ("uspekhi matematicheskikh nauk", "Uspekhi Mat. Nauk"),
    ("uspehi mat nauk", "Uspehi Mat. Nauk"),
    (
        "zeitschrift fur analysis und ihre anwendungen",
        "Z. Anal. Anwend.",
    ),
    ("zeitschrift fur angewandte mathematik und physik", "Z. Angew. Math. Phys."),
    ("zeitschrift fur mathematische logik und grundlagen der mathematik", "Z. Math. Logik Grundlag. Math."),
    ("zeitschrift fur wahrscheinlichkeitstheorie und verwandte gebiete", "Z. Wahrsch. Verw. Gebiete"),
];

const ISO4_STOPWORDS: &[&str] = &[
    "a", "an", "and", "at", "by", "de", "dei", "del", "della", "delle", "degli", "der", "des",
    "di", "die", "das", "du", "e", "ed", "et", "for", "from", "fuer", "fur", "im", "in", "into",
    "its", "l", "la", "le", "les", "of", "on", "or", "over", "the", "their", "to", "und", "vom",
    "von", "with", "zur", "zum",
];

// ISO-4 / AMS title-word abbreviations (lowercase ASCII keys -> canonical abbreviated token).
const ISO4_WORD_MAP: &[(&str, &str)] = &[
    ("abstract", "Abstr."),
    ("academia", "Acad."),
    ("academiae", "Acad."),
    ("academie", "Acad."),
    ("academy", "Acad."),
    ("advances", "Adv."),
    ("algebraic", "Algebr."),
    ("algoritm", "Algorithm"),
    ("algorithms", "Algorithms"),
    ("allgemeine", "Allg."),
    ("american", "Amer."),
    ("analise", "Anal."),
    ("analyse", "Anal."),
    ("analysis", "Anal."),
    ("analitica", "Anal."),
    ("analytical", "Anal."),
    ("analytic", "Anal."),
    ("angewandte", "Angew."),
    ("annalen", "Ann."),
    ("annales", "Ann."),
    ("annali", "Ann."),
    ("annals", "Ann."),
    ("applications", "Appl."),
    ("applied", "Appl."),
    ("applicata", "Appl."),
    ("appliquees", "Appl."),
    ("approximation", "Approx."),
    ("archival", "Arch."),
    ("archive", "Arch."),
    ("archives", "Arch."),
    ("arithmetica", "Arith."),
    ("arithmetic", "Arith."),
    ("arkiv", "Ark."),
    ("artificial", "Artif."),
    ("association", "Assoc."),
    ("asymptotic", "Asymptot."),
    ("australian", "Aust."),
    ("automatic", "Automat."),
    ("biomedical", "Biomed."),
    ("boundary", "Boundary"),
    ("british", "Brit."),
    ("bulletin", "Bull."),
    ("bulletins", "Bull."),
    ("bollettino", "Boll."),
    ("calculus", "Calc."),
    ("cambridge", "Camb."),
    ("canadian", "Canad."),
    ("central", "Cent."),
    ("chemical", "Chem."),
    ("chemistry", "Chem."),
    ("chinese", "Chin."),
    ("combinatorial", "Combin."),
    ("combinatorics", "Combin."),
    ("commentarii", "Comment."),
    ("commentationes", "Comment."),
    ("communications", "Commun."),
    ("communication", "Commun."),
    ("complex", "Complex"),
    ("complexity", "Complexity"),
    ("compositio", "Compos."),
    ("computation", "Comput."),
    ("computational", "Comput."),
    ("computer", "Comput."),
    ("computers", "Comput."),
    ("computing", "Comput."),
    ("conference", "Conf."),
    ("constructive", "Constr."),
    ("contemporary", "Contemp."),
    ("continuous", "Contin."),
    ("control", "Control"),
    ("cryptology", "Cryptol."),
    ("differential", "Differ."),
    ("discrete", "Discrete"),
    ("documenta", "Doc."),
    ("dynamical", "Dyn."),
    ("dynamics", "Dyn."),
    ("ecological", "Ecol."),
    ("economics", "Econ."),
    ("edinburgh", "Edinb."),
    ("education", "Educ."),
    ("educational", "Educ."),
    ("electrical", "Electr."),
    ("electronic", "Electron."),
    ("electronics", "Electron."),
    ("engineering", "Engrg."),
    ("equations", "Equ."),
    ("equation", "Equ."),
    ("ergodic", "Ergodic"),
    ("european", "Eur."),
    ("evolution", "Evol."),
    ("experimental", "Exp."),
    ("experiments", "Exp."),
    ("expositions", "Expo."),
    ("financial", "Financ."),
    ("fluid", "Fluid"),
    ("foundations", "Found."),
    ("fourier", "Fourier"),
    ("fractional", "Fract."),
    ("french", "Fr."),
    ("francaise", "Fr."),
    ("functional", "Funct."),
    ("functions", "Funct."),
    ("fundamenta", "Fund."),
    ("fundamental", "Fund."),
    ("geometric", "Geom."),
    ("geometrical", "Geom."),
    ("geometrie", "Geom."),
    ("geometry", "Geom."),
    ("german", "Ger."),
    ("glasgow", "Glasg."),
    ("global", "Global"),
    ("graphical", "Graph."),
    ("harmonic", "Harmon."),
    ("helvetici", "Helv."),
    ("history", "Hist."),
    ("hydrodynamics", "Hydrodyn."),
    ("illinois", "Ill."),
    ("imaging", "Imaging"),
    ("indiana", "Indiana"),
    ("industrial", "Ind."),
    ("information", "Inf."),
    ("informatique", "Inform."),
    ("institute", "Inst."),
    ("institut", "Inst."),
    ("integral", "Integr."),
    ("intelligence", "Intell."),
    ("international", "Int."),
    ("inventiones", "Invent."),
    ("italian", "Ital."),
    ("italiana", "Ital."),
    ("japanese", "Jpn."),
    ("journal", "J."),
    ("journals", "J."),
    ("kinetic", "Kinet."),
    ("lecture", "Lect."),
    ("lectures", "Lect."),
    ("letters", "Lett."),
    ("letter", "Lett."),
    ("linear", "Linear"),
    ("logic", "Logic"),
    ("logical", "Log."),
    ("london", "Lond."),
    ("machine", "Mach."),
    ("machines", "Mach."),
    ("manuscripta", "Manuscripta"),
    ("matematica", "Mat."),
    ("matematiche", "Mat."),
    ("matematici", "Mat."),
    ("matematik", "Mat."),
    ("matematikai", "Mat."),
    ("mathematica", "Math."),
    ("mathematicae", "Math."),
    ("mathematical", "Math."),
    ("mathematici", "Math."),
    ("mathematics", "Math."),
    ("mathematik", "Math."),
    ("mathematique", "Math."),
    ("mathematiques", "Math."),
    ("mathematische", "Math."),
    ("mechanics", "Mech."),
    ("mechanica", "Mech."),
    ("mechanical", "Mech."),
    ("medical", "Med."),
    ("medicine", "Med."),
    ("mediterranean", "Mediterr."),
    ("memoires", "Mém."),
    ("memoirs", "Mem."),
    ("methods", "Methods"),
    ("michigan", "Mich."),
    ("modeling", "Model."),
    ("modelling", "Model."),
    ("models", "Models"),
    ("modern", "Mod."),
    ("monatshefte", "Monatsh."),
    ("monthly", "Monthly"),
    ("multiscale", "Multiscale"),
    ("nachrichten", "Nachr."),
    ("nagoya", "Nagoya"),
    ("national", "Natl."),
    ("natural", "Nat."),
    ("networks", "Netw."),
    ("nonlinear", "Nonlinear"),
    ("notes", "Notes"),
    ("notices", "Notices"),
    ("numerical", "Numer."),
    ("numerische", "Numer."),
    ("operator", "Oper."),
    ("operators", "Oper."),
    ("optical", "Opt."),
    ("optimization", "Optim."),
    ("ordinary", "Ord."),
    ("pacific", "Pac."),
    ("partial", "Partial"),
    ("pattern", "Pattern"),
    ("physical", "Phys."),
    ("physics", "Phys."),
    ("physik", "Phys."),
    ("physique", "Phys."),
    ("polish", "Pol."),
    ("polonici", "Polon."),
    ("potential", "Potential"),
    ("probabilites", "Probab."),
    ("probability", "Probab."),
    ("problems", "Probl."),
    ("proceedings", "Proc."),
    ("process", "Process."),
    ("processes", "Process."),
    ("processing", "Process."),
    ("programming", "Program."),
    ("progress", "Progr."),
    ("publications", "Publ."),
    ("pure", "Pure"),
    ("pura", "Pura"),
    ("pures", "Pures"),
    ("quarterly", "Q."),
    ("quantum", "Quantum"),
    ("rational", "Ration."),
    ("recherche", "Rech."),
    ("records", "Rec."),
    ("rendiconti", "Rend."),
    ("reports", "Rep."),
    ("representations", "Represent."),
    ("representation", "Represent."),
    ("research", "Res."),
    ("review", "Rev."),
    ("reviews", "Rev."),
    ("revista", "Rev."),
    ("revue", "Rev."),
    ("royal", "R."),
    ("russian", "Russ."),
    ("scandinavica", "Scand."),
    ("science", "Sci."),
    ("sciences", "Sci."),
    ("scientific", "Sci."),
    ("scientifiques", "Sci."),
    ("scuola", "Sc."),
    ("section", "Sect."),
    ("series", "Ser."),
    ("siberian", "Sib."),
    ("signal", "Signal"),
    ("simulation", "Simul."),
    ("societe", "Soc."),
    ("society", "Soc."),
    ("spectral", "Spectr."),
    ("statistical", "Stat."),
    ("statistics", "Stat."),
    ("statistiques", "Stat."),
    ("stochastic", "Stoch."),
    ("stochastics", "Stoch."),
    ("structures", "Struct."),
    ("studies", "Stud."),
    ("studia", "Stud."),
    ("superiore", "Super."),
    ("superieure", "Supér."),
    ("supplement", "Suppl."),
    ("surveys", "Surv."),
    ("symbolic", "Symb."),
    ("symplectic", "Symplectic"),
    ("symposium", "Symp."),
    ("system", "Syst."),
    ("systems", "Syst."),
    ("technical", "Tech."),
    ("theoretical", "Theor."),
    ("theorie", "Théor."),
    ("topology", "Topol."),
    ("topological", "Topol."),
    ("transactions", "Trans."),
    ("transformation", "Transform."),
    ("trends", "Trends"),
    ("universita", "Univ."),
    ("university", "Univ."),
    ("variational", "Var."),
    ("variations", "Var."),
    ("vision", "Vis."),
    ("zeitschrift", "Z."),
];

/// Returns `true` for preprint archives/repositories or empty strings that should never
/// be rewritten as abbreviated journals.
pub fn is_preprint_or_generic_venue(raw: &str) -> bool {
    let norm = normalize_math_text(raw);
    if norm.is_empty() {
        return true;
    }
    matches!(
        norm.as_str(),
        "arxiv"
            | "arxiv org"
            | "corr"
            | "biorxiv"
            | "medrxiv"
            | "chemrxiv"
            | "ssrn"
            | "hal"
            | "zenodo"
            | "figshare"
            | "preprint"
            | "working paper"
    ) || norm.contains("arxiv")
        || norm.contains("biorxiv")
        || norm.contains("medrxiv")
        || norm.contains("ssrn")
}

/// Checks whether `raw` already appears to be an abbreviated journal title (contains at least
/// one genuine word-abbreviation dot such as `Math.` or `J.`, or matches an invariant
/// single-word/canonical short title such as `Nature` or `Topology`).
pub fn looks_abbreviated_journal(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.is_empty() || is_preprint_or_generic_venue(trimmed) {
        return false;
    }
    let norm = normalize_math_text(trimmed);
    if DOTLESS_CANONICAL_JOURNALS.contains(&norm.as_str()) {
        return true;
    }
    if CURATED_JOURNAL_ABBREVS
        .iter()
        .any(|(_, abbr)| normalize_math_text(abbr) == norm)
    {
        return true;
    }

    for tok in trimmed.split_whitespace() {
        let tok_clean = tok.trim_matches(|c: char| matches!(c, ',' | ';' | '(' | ')' | '[' | ']'));
        if let Some(stem) = tok_clean.strip_suffix('.') {
            if !stem.is_empty()
                && stem.chars().count() <= 14
                && stem.chars().all(|c| c.is_alphabetic() || c == '-')
            {
                return true;
            }
        }
    }
    false
}

/// Looks up an authoritative mathematical/scientific journal abbreviation from the built-in
/// curated table (`CURATED_JOURNAL_ABBREVS`).
pub fn lookup_curated_journal_abbrev(raw: &str) -> Option<&'static str> {
    let norm = normalize_math_text(raw);
    if norm.is_empty() {
        return None;
    }
    if let Some((_, abbr)) = CURATED_JOURNAL_ABBREVS.iter().find(|(k, _)| *k == norm) {
        return Some(*abbr);
    }
    // Also tolerate subtitles stripped after `:` or ` - `
    if let Some(head) = raw.split(':').next() {
        let head_norm = normalize_math_text(head);
        if !head_norm.is_empty() && head_norm != norm {
            if let Some((_, abbr)) = CURATED_JOURNAL_ABBREVS.iter().find(|(k, _)| *k == head_norm) {
                return Some(*abbr);
            }
        }
    }
    None
}

/// Deterministically abbreviates a full journal title using ISO-4 / AMS title-word rules
/// as a fallback when no exact curated or online serial abbreviation is found.
pub fn iso4_abbreviate_journal(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || is_preprint_or_generic_venue(trimmed) {
        return None;
    }
    let head = trimmed.split(':').next().unwrap_or(trimmed).trim();
    if head.is_empty() {
        return None;
    }

    // Split hyphenless tokens cleanly while separating French/Italian elisions (`l'`, `d'`, `dell'`)
    let pre_split = head
        .replace('\u{2019}', "'")
        .replace("l'", " ")
        .replace("L'", " ")
        .replace("d'", " ")
        .replace("D'", " ")
        .replace("dell'", " ")
        .replace("Dell'", " ");

    let raw_words: Vec<&str> = pre_split.split_whitespace().collect();
    if raw_words.len() <= 1 {
        return None;
    }

    let mut out_tokens: Vec<String> = Vec::with_capacity(raw_words.len());
    let mut abbreviated_count = 0usize;

    for (idx, word) in raw_words.iter().enumerate() {
        let clean = word.trim_matches(|c: char| matches!(c, ',' | ';' | '&'));
        if clean.is_empty() {
            continue;
        }
        if clean.starts_with('(') && clean.ends_with(')') {
            out_tokens.push(clean.to_string());
            continue;
        }
        let low_norm = normalize_math_text(clean);
        if low_norm.is_empty() {
            continue;
        }
        let is_first_or_last = idx == 0 || idx + 1 == raw_words.len();
        if !is_first_or_last && ISO4_STOPWORDS.contains(&low_norm.as_str()) {
            continue;
        }
        if let Some((_, abbr)) = ISO4_WORD_MAP.iter().find(|(k, _)| *k == low_norm) {
            if abbr.ends_with('.') && *abbr != clean {
                abbreviated_count += 1;
            }
            out_tokens.push((*abbr).to_string());
        } else {
            out_tokens.push(clean.to_string());
        }
    }

    if out_tokens.is_empty() || abbreviated_count == 0 {
        return None;
    }
    let candidate = out_tokens.join(" ");
    if candidate.chars().count() >= trimmed.chars().count() {
        return None;
    }
    if normalize_math_text(&candidate) == normalize_math_text(trimmed) {
        return None;
    }
    Some(candidate)
}

/// Resolves a journal abbreviation locally from curated dictionary first, then ISO-4 rules.
pub fn resolve_local_journal_abbrev(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || is_preprint_or_generic_venue(trimmed) {
        return None;
    }
    if let Some(curated) = lookup_curated_journal_abbrev(trimmed) {
        return Some(curated.to_string());
    }
    iso4_abbreviate_journal(trimmed)
}

/// Returns the candidate full journal name that needs abbreviation resolution if `rec.journal_abbrev`
/// is missing or does not yet appear genuinely abbreviated; returns `None` if `rec.journal_abbrev`
/// is already properly abbreviated.
pub fn journal_name_needing_abbrev(rec: &BibliographicRecord) -> Option<String> {
    let cur_abbr = rec
        .journal_abbrev
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let cur_full = rec
        .journal_full
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    match (cur_abbr, cur_full) {
        (Some(abbr), Some(full)) => {
            if is_preprint_or_generic_venue(abbr) && is_preprint_or_generic_venue(full) {
                return None;
            }
            if !looks_abbreviated_journal(abbr) {
                return Some(full.to_string());
            }
            if normalize_math_text(abbr) == normalize_math_text(full) {
                if let Some(curated) = lookup_curated_journal_abbrev(full) {
                    if normalize_math_text(curated) != normalize_math_text(abbr) {
                        return Some(full.to_string());
                    }
                }
            }
            None
        }
        (Some(abbr), None) => {
            if is_preprint_or_generic_venue(abbr) || looks_abbreviated_journal(abbr) {
                None
            } else {
                Some(abbr.to_string())
            }
        }
        (None, Some(full)) => {
            if is_preprint_or_generic_venue(full) {
                None
            } else {
                Some(full.to_string())
            }
        }
        (None, None) => None,
    }
}

/// Promotes an unabbreviated `journal_abbrev` into `journal_full` (when `journal_full` is empty)
/// and applies curated dictionary lookup if matched. Does NOT run heuristic ISO-4 fallback yet so
/// callers can query online serial authorities (e.g. zbMATH Serials) before falling back to ISO-4.
pub fn normalize_record_curated_journal(rec: &mut BibliographicRecord, lock_abbrev: bool) {
    if lock_abbrev {
        return;
    }
    // If an unabbreviated title was mistakenly placed only in journal_abbrev, preserve it in journal_full.
    if rec.journal_full.as_deref().map(str::trim).unwrap_or("").is_empty() {
        if let Some(abbr) = rec.journal_abbrev.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            if !looks_abbreviated_journal(abbr) && !is_preprint_or_generic_venue(abbr) {
                rec.journal_full = Some(abbr.to_string());
            }
        }
    }
    let Some(target_full) = journal_name_needing_abbrev(rec) else {
        return;
    };
    if let Some(curated) = lookup_curated_journal_abbrev(&target_full) {
        rec.journal_abbrev = Some(curated.to_string());
    }
}

/// Applies deterministic ISO-4 abbreviation fallback if `rec.journal_abbrev` is still missing or
/// unabbreviated.
pub fn apply_iso4_journal_fallback(rec: &mut BibliographicRecord, lock_abbrev: bool) {
    if lock_abbrev {
        return;
    }
    let Some(target_full) = journal_name_needing_abbrev(rec) else {
        return;
    };
    if let Some(abbr) = resolve_local_journal_abbrev(&target_full) {
        rec.journal_abbrev = Some(abbr);
    }
}

/// Convenience helper for offline/local normalization: preserves unabbreviated full name in
/// `journal_full` and resolves `journal_abbrev` via curated table + ISO-4 rules.
pub fn normalize_record_journals_locally(rec: &mut BibliographicRecord, lock_abbrev: bool) {
    normalize_record_curated_journal(rec, lock_abbrev);
    apply_iso4_journal_fallback(rec, lock_abbrev);
}

/// Checks if a serial title returned by an external catalog matches `query_name` closely enough
/// to trust its abbreviation without an ISSN match.
pub fn serial_titles_match(candidate_title: &str, query_name: &str) -> bool {
    let a = normalize_math_text(candidate_title);
    let b = normalize_math_text(query_name);
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b {
        return true;
    }
    let strip_leading_the = |s: &str| -> String {
        s.strip_prefix("the ").unwrap_or(s).to_string()
    };
    let a_s = strip_leading_the(&a);
    let b_s = strip_leading_the(&b);
    if a_s == b_s {
        return true;
    }
    let a_toks: HashSet<&str> = a_s
        .split_whitespace()
        .filter(|w| !ISO4_STOPWORDS.contains(w))
        .collect();
    let b_toks: HashSet<&str> = b_s
        .split_whitespace()
        .filter(|w| !ISO4_STOPWORDS.contains(w))
        .collect();
    if a_toks.is_empty() || b_toks.is_empty() {
        return false;
    }
    let inter = a_toks.intersection(&b_toks).count();
    let union = a_toks.union(&b_toks).count();
    inter * 10 >= union * 9
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::models::WorkType;

    #[test]
    fn curated_lookup_abbreviates_major_math_journals_and_preserves_full_field() {
        let mut rec = BibliographicRecord {
            work_type: WorkType::Article,
            title: "Test Paper".into(),
            journal_abbrev: Some("Inventiones Mathematicae".into()),
            journal_full: None,
            ..Default::default()
        };
        normalize_record_journals_locally(&mut rec, false);
        assert_eq!(rec.journal_full.as_deref(), Some("Inventiones Mathematicae"));
        assert_eq!(rec.journal_abbrev.as_deref(), Some("Invent. Math."));

        let mut rec2 = BibliographicRecord {
            work_type: WorkType::Article,
            title: "Test Paper 2".into(),
            journal_abbrev: None,
            journal_full: Some("Transactions of the American Mathematical Society".into()),
            ..Default::default()
        };
        normalize_record_journals_locally(&mut rec2, false);
        assert_eq!(rec2.journal_abbrev.as_deref(), Some("Trans. Amer. Math. Soc."));
    }

    #[test]
    fn iso4_fallback_abbreviates_unlisted_journals_and_keeps_existing_dotted_abbrev() {
        let mut rec = BibliographicRecord {
            work_type: WorkType::Article,
            title: "Exotic Journal Article".into(),
            journal_abbrev: Some("Mediterranean Journal of Functional Analysis and Optimization".into()),
            journal_full: None,
            ..Default::default()
        };
        normalize_record_journals_locally(&mut rec, false);
        assert_eq!(
            rec.journal_full.as_deref(),
            Some("Mediterranean Journal of Functional Analysis and Optimization")
        );
        assert_eq!(
            rec.journal_abbrev.as_deref(),
            Some("Mediterr. J. Funct. Anal. Optim.")
        );

        let mut mr_rec = BibliographicRecord {
            work_type: WorkType::Article,
            title: "Golden Article".into(),
            journal_abbrev: Some("Ann. of Math. (2)".into()),
            journal_full: Some("Annals of Mathematics. Second Series".into()),
            ..Default::default()
        };
        normalize_record_journals_locally(&mut mr_rec, false);
        assert_eq!(mr_rec.journal_abbrev.as_deref(), Some("Ann. of Math. (2)"));
    }

    #[test]
    fn respects_locked_abbrev_and_skips_preprints() {
        let mut locked = BibliographicRecord {
            work_type: WorkType::Article,
            title: "Locked Test".into(),
            journal_abbrev: Some("Inventiones Mathematicae".into()),
            ..Default::default()
        };
        normalize_record_journals_locally(&mut locked, true);
        assert_eq!(locked.journal_abbrev.as_deref(), Some("Inventiones Mathematicae"));

        let mut arxiv = BibliographicRecord {
            work_type: WorkType::Misc,
            title: "Preprint".into(),
            journal_abbrev: None,
            journal_full: Some("arXiv.org".into()),
            ..Default::default()
        };
        normalize_record_journals_locally(&mut arxiv, false);
        assert_eq!(arxiv.journal_abbrev, None);
    }
}
