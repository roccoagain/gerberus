Hand-written answers in the shape of Mouser's Search API (`/api/v1/search/partnumber`) and DigiKey's Product
Information v4 (`/products/v4/search/{part}/productdetails`), for the parsers in `src/bom.rs`. They weren't
recorded from the live APIs, which the development environment can't reach: replace them with recorded answers
when someone with a key runs `gerberus bom --from mouser|digikey --cache DIR --max-age 0`. Stock numbers are
made up.
