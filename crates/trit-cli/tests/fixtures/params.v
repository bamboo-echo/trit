module leaf #(parameter W = 2) (
    input [W-1:0] a,
    input [W-1:0] b,
    output [W-1:0] y
);
    assign y = a & b;
endmodule

module top #(parameter W = 3) (
    input [W-1:0] a,
    input [W-1:0] b,
    output [W-1:0] y
);
    leaf #(.W(W)) u (.a(a), .b(b), .y(y));
endmodule
