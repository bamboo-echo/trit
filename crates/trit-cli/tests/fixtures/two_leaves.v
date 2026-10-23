module leaf(input a, input b, output y);
assign y = a & b;
endmodule
module top(input a, input b, input c, input d, output y1, output y2);
leaf u1 (.a(a), .b(b), .y(y1));
leaf u2 (.a(c), .b(d), .y(y2));
endmodule
