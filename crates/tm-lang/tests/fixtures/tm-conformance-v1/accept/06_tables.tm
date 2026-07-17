table [{name: "miku", age: 21}, {name: "ice", age: 30}]
|> where (age > 18)
|> select {name}
|> sort_by name asc
