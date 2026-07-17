let 名字 = "Miku";
fun greet who = "hello #who";
do { let message = greet 名字; display {kind: "text"} message };
