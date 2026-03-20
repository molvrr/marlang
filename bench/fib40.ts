const fib = (n: number) => n < 2 ? n : fib(n - 1) + fib(n - 2)

console.log(fib(40))
